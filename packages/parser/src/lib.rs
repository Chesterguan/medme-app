use chrono::{DateTime, TimeZone, Utc};
use core_model::DocType;
use regex::Regex;
use std::path::Path;
use std::sync::OnceLock;

mod labs;
pub use labs::{extract_labs, LabObservation};

pub struct Extracted {
    pub text: String,
    pub page_count: i32,
    pub language: Option<String>,
    pub doc_date: Option<DateTime<Utc>>,
    pub doc_date_end: Option<DateTime<Utc>>, // 区间结束(住院类文档);单点文档为 None
    pub doc_type: DocType,
}

pub fn extract(path: &Path) -> anyhow::Result<Extracted> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let (text, page_count) = match ext.as_str() {
        "txt" => (std::fs::read_to_string(path)?, 1),
        "pdf" => {
            // pdf-extract can PANIC (internal unwrap/index/`unimplemented!`) on
            // malformed/truncated PDFs rather than returning Err, and that panic
            // would unwind past the caller's non-fatal fallback and crash the
            // process. Catch it here and convert to a normal Err so the existing
            // fallback (store the file without extracted text) engages. Valid
            // PDFs are unaffected.
            let t = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                pdf_extract::extract_text(path)
            }))
            .map_err(|_| anyhow::anyhow!("pdf_extract panicked on malformed PDF"))??;
            let pages = t.matches('\u{0C}').count() as i32 + 1; // 换页符估页数
            (t, pages)
        }
        other => anyhow::bail!("unsupported extension: {other}"),
    };
    let (doc_date, doc_date_end) = guess_date_range(&text);
    Ok(Extracted {
        language: detect_language(&text),
        doc_date,
        doc_date_end,
        doc_type: classify(&text),
        text,
        page_count,
    })
}

pub fn detect_language(text: &str) -> Option<String> {
    let has_cjk = text.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c));
    let has_latin = text.chars().any(|c| c.is_ascii_alphabetic());
    match (has_cjk, has_latin) {
        (true, true) => Some("mixed".into()),
        (true, false) => Some("zh".into()),
        (false, true) => Some("en".into()),
        (false, false) => None,
    }
}

/// 抽取文本中所有合法日期,返回 (最早, 最晚)。最晚为 None 当只有一个不同日期时。
/// 用于住院等跨度文档:入院=起、出院=止;单点文档 end=None。
fn iso_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(\d{4})[-/.](\d{1,2})[-/.](\d{1,2})").expect("iso date re"))
}
fn cn_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(\d{4})\s*年\s*(\d{1,2})\s*月\s*(\d{1,2})\s*日").expect("cn date re")
    })
}

/// 某字符串里第一个合法日期(ISO 优先,再中文);拒绝嵌在更长数字串里的"日期"(如住院号里的)。
fn first_date_in(s: &str) -> Option<DateTime<Utc>> {
    for caps in iso_re().captures_iter(s) {
        let m = caps.get(0).expect("group 0");
        let before = s[..m.start()]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_digit());
        let after = s[m.end()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit());
        if before || after {
            continue;
        }
        if let Some(d) = build_date(&caps) {
            return Some(d);
        }
    }
    cn_re().captures_iter(s).find_map(|c| build_date(&c))
}

/// 全文所有合法日期,排序去重。
fn all_dates_in(text: &str) -> Vec<DateTime<Utc>> {
    let mut v: Vec<DateTime<Utc>> = Vec::new();
    for caps in iso_re().captures_iter(text) {
        let m = caps.get(0).expect("group 0");
        let before = text[..m.start()]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_digit());
        let after = text[m.end()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit());
        if before || after {
            continue;
        }
        if let Some(d) = build_date(&caps) {
            v.push(d);
        }
    }
    for caps in cn_re().captures_iter(text) {
        if let Some(d) = build_date(&caps) {
            v.push(d);
        }
    }
    v.sort();
    v.dedup();
    v
}

/// 标签后紧跟的日期(标签后约 24 字符窗口内)。
fn labeled_date(text: &str, labels: &[&str]) -> Option<DateTime<Utc>> {
    for lbl in labels {
        if let Some(pos) = text.find(lbl) {
            let after: String = text[pos + lbl.len()..].chars().take(24).collect();
            if let Some(d) = first_date_in(&after) {
                return Some(d);
            }
        }
    }
    None
}

/// 文档日期:优先"带标签的报告日期"(避免抓到正文引用的历史日期),住院取 入院→出院 区间,
/// 都没有才回退全文最早/最晚。
pub fn guess_date_range(text: &str) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    // 1) 住院:入院 + 出院 → 区间
    let admit = labeled_date(text, &["入院日期", "入院时间"]);
    let discharge = labeled_date(text, &["出院日期", "出院时间"]);
    if let (Some(a), Some(d)) = (admit, discharge) {
        return (Some(a), Some(d));
    }
    // 2) 报告类标签(文档自己的日期,优先于正文引用的历史日期)
    let report = labeled_date(
        text,
        &[
            "检查日期",
            "检测日期",
            "检验日期",
            "报告日期",
            "报告时间",
            "就诊日期",
            "就诊时间",
            "收到日期",
            "采集时间",
            "采集日期",
            "签发日期",
            "手术日期",
            "检查时间",
            "诊断日期",
        ],
    );
    if let Some(d) = report {
        return (Some(d), None);
    }
    // 3) 单个 入院/出院,或泛"日期"标签
    if let Some(d) = admit.or(discharge) {
        return (Some(d), None);
    }
    if let Some(d) = labeled_date(text, &["日期"]) {
        return (Some(d), None);
    }
    // 4) 回退:全文最早/最晚
    let all = all_dates_in(text);
    match all.len() {
        0 => (None, None),
        1 => (Some(all[0]), None),
        _ => (Some(all[0]), Some(all[all.len() - 1])),
    }
}

/// 单点日期(向后兼容):取文档日期。
pub fn guess_date(text: &str) -> Option<DateTime<Utc>> {
    guess_date_range(text).0
}

fn build_date(caps: &regex::Captures) -> Option<DateTime<Utc>> {
    let y: i32 = caps.get(1)?.as_str().parse().ok()?;
    let m: u32 = caps.get(2)?.as_str().parse().ok()?;
    let d: u32 = caps.get(3)?.as_str().parse().ok()?;
    if !(1900..=2100).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).single()
}

pub struct Demographics {
    pub name: Option<String>,
    pub gender: Option<String>,     // "男" / "女"
    pub birth_date: Option<String>, // RFC3339 date if 出生日期/生日 present
    pub age: Option<String>,        // 年龄数字(字符串)
}

pub fn extract_demographics(text: &str) -> Demographics {
    static NAME: OnceLock<Regex> = OnceLock::new();
    static GENDER: OnceLock<Regex> = OnceLock::new();
    static AGE: OnceLock<Regex> = OnceLock::new();
    static BIRTH: OnceLock<Regex> = OnceLock::new();
    let name = NAME.get_or_init(|| {
        Regex::new(r"(?:姓名|名字)[:：]\s*([^\s，,;；、\d]{1,10})").expect("name regex")
    });
    let gender = GENDER.get_or_init(|| Regex::new(r"性别[:：]\s*([男女])").expect("gender regex"));
    let age = AGE.get_or_init(|| Regex::new(r"年龄[:：]\s*(\d{1,3})").expect("age regex"));
    let birth = BIRTH.get_or_init(|| {
        Regex::new(r"(?:出生日期|出生|生日)[:：]\s*(\d{4})[-/.](\d{1,2})[-/.](\d{1,2})")
            .expect("birth regex")
    });
    let cap1 = |re: &Regex| {
        re.captures(text)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
    };
    let birth_date = birth
        .captures(text)
        .map(|c| format!("{}-{:0>2}-{:0>2}", &c[1], &c[2], &c[3]));
    Demographics {
        name: cap1(name),
        gender: cap1(gender),
        birth_date,
        age: cap1(age),
    }
}

pub fn classify(text: &str) -> DocType {
    let lower = text.to_lowercase(); // 拉丁字母小写化;中文不变,仍能匹配
    let has = |kw: &str| lower.contains(kw);
    // 短英文缩写用整词匹配,避免 "doctor"(含 ct)/"available"(含 lab)误命中
    let words: std::collections::HashSet<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();
    let word = |w: &str| words.contains(w);

    if has("出院记录") || has("discharge") {
        DocType::DischargeSummary
    } else if has("处方") || has("prescription") {
        DocType::Prescription
    } else if has("手术记录")
        || has("手术经过")
        || has("术中")
        || has("surgical")
        || has("operative")
    {
        DocType::Surgery
    } else if has("检验") || has("化验") || has("laborator") || word("lab") {
        DocType::LabReport
    } else if has("门诊病历")
        || has("门诊记录")
        || has("急诊病历")
        || has("急诊记录")
        || has("入院记录")
        || has("病程记录")
        || has("门诊电子病历")
    {
        // 病历/门急诊记录:文档自己声明的类型优先于正文里顺带提到的 CT/MRI
        DocType::ClinicalNote
    } else if has("影像")
        || has("超声")
        || has("ultrasound")
        || has("imaging")
        || has("radiolog")
        || has("computed tomograph")
        || has("magnetic resonance")
        || word("ct")
        || word("mri")
        || word("xray")
        || has("x-ray")
        || has("x线")
        || has("心电")
        || has("dr ")
        || has("拍片")
    {
        DocType::ImagingReport
    } else if has("病理") || has("pathology") {
        DocType::Pathology
    } else if has("血压记录")
        || has("血糖记录")
        || has("体温记录")
        || has("家庭监测")
        || has("血压监测")
        || has("血糖监测")
        || has("自测")
    {
        DocType::Other
    } else {
        DocType::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_txt_fixture() {
        let p = std::path::Path::new("tests/fixtures/sample.txt");
        let e = extract(p).unwrap();
        assert!(e.text.contains("Creatinine"));
        assert_eq!(e.page_count, 1);
        assert_eq!(e.language.as_deref(), Some("mixed"));
        assert_eq!(e.doc_type, core_model::DocType::DischargeSummary);
        assert_eq!(
            e.doc_date.unwrap().format("%Y-%m-%d").to_string(),
            "2023-05-01"
        );
    }

    /// Builds a structurally-valid single-page PDF (parseable by lopdf) whose
    /// content stream is `content`. When `include_font` is false the page's
    /// Resources have no `/Font`, so a text-show operator referencing `/F1`
    /// drives pdf-extract into an internal panic during text extraction.
    fn build_pdf(content: &[u8], include_font: bool) -> Vec<u8> {
        let font_res: &[u8] = if include_font {
            b"/Font << /F1 5 0 R >>"
        } else {
            b""
        };
        let mut objs: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        ];
        let mut page =
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << "
                .to_vec();
        page.extend_from_slice(font_res);
        page.extend_from_slice(b" >> >>");
        objs.push(page);
        let mut stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
        stream.extend_from_slice(content);
        stream.extend_from_slice(b"\nendstream");
        objs.push(stream);
        if include_font {
            objs.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
        }

        let mut out = b"%PDF-1.5\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref_start = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n", objs.len() + 1).as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for off in &offsets {
            out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF",
                objs.len() + 1,
                xref_start
            )
            .as_bytes(),
        );
        out
    }

    fn write_pdf(bytes: &[u8]) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut f = tempfile::Builder::new()
            .suffix(".pdf")
            .tempfile()
            .expect("temp pdf");
        f.write_all(bytes).expect("write pdf");
        f.flush().expect("flush");
        f
    }

    #[test]
    fn malformed_pdf_returns_err_not_panic() {
        // This structurally-valid PDF shows text with a font missing from the
        // page's Resources, which makes pdf-extract PANIC during text
        // extraction (not return Err). Without the catch_unwind firewall in
        // `extract`, that panic unwinds past the caller's non-fatal fallback and
        // aborts this test process; with it, `extract` returns a normal Err.
        let panicking = build_pdf(b"BT /F1 12 Tf (Hello) Tj ET", false);
        let f = write_pdf(&panicking);
        assert!(
            extract(f.path()).is_err(),
            "panic-inducing PDF should return Err, not crash the process"
        );

        // A truncated PDF (pdf-extract returns Err here) still errors cleanly.
        let truncated = write_pdf(b"%PDF-1.5\n1 0 obj\n<< /Type /Catalog");
        assert!(extract(truncated.path()).is_err());
    }

    #[test]
    fn valid_pdf_still_extracts_text() {
        // Regression guard: the firewall must not change behavior for valid PDFs.
        let good = build_pdf(b"BT /F1 12 Tf (Hello) Tj ET", true);
        let f = write_pdf(&good);
        let e = extract(f.path()).expect("valid PDF should parse");
        assert!(e.text.contains("Hello"), "unexpected text: {:?}", e.text);
        assert_eq!(e.page_count, 1);
    }

    #[test]
    fn language_detection() {
        assert_eq!(detect_language("hello world").as_deref(), Some("en"));
        assert_eq!(detect_language("你好世界").as_deref(), Some("zh"));
        assert_eq!(detect_language("hello 世界").as_deref(), Some("mixed"));
    }

    #[test]
    fn cn_date_parses_and_never_panics() {
        // 合法中文日期
        let d = guess_date("检查日期 2021年3月4日 完成").unwrap();
        assert_eq!(d.format("%Y-%m-%d").to_string(), "2021-03-04");
        // 日 在 月 之前(如 "每日…X月")→ 不 panic,返回 None
        assert!(guess_date("2020年3日4月").is_none());
        // 只有 年,无 月日 → None
        assert!(guess_date("2020年记录").is_none());
    }

    #[test]
    fn guess_date_handles_labeled_and_age_confusion() {
        // 日期粘在标签后(冒号无空格)
        assert_eq!(
            guess_date("出院日期:2023-05-01    科室:神经内科")
                .unwrap()
                .format("%Y-%m-%d")
                .to_string(),
            "2023-05-01"
        );
        // 文本含"年龄"(含"年")时,中文日期仍需正确解析
        let t = "姓名:张三 年龄:60岁 检查日期:2025年02月18日 影像所见";
        assert_eq!(
            guess_date(t).unwrap().format("%Y-%m-%d").to_string(),
            "2025-02-18"
        );
        // 斜杠格式,带时间后缀
        assert_eq!(
            guess_date("采集 2024/01/15 07:52")
                .unwrap()
                .format("%Y-%m-%d")
                .to_string(),
            "2024-01-15"
        );
        // 空占位符 → 无有效日期
        assert!(guess_date("检测日期:____年__月__日").is_none());
    }

    #[test]
    fn classify_case_insensitive_and_english() {
        assert_eq!(
            classify("Discharge Summary\nDiagnosis: pneumonia"),
            DocType::DischargeSummary
        );
        assert_eq!(
            classify("Laboratory Report\nHemoglobin 140"),
            DocType::LabReport
        );
        assert_eq!(
            classify("Ultrasound Report\nfatty liver"),
            DocType::ImagingReport
        );
        assert_eq!(
            classify("chest CT scan report\nnodule"),
            DocType::ImagingReport
        );
        // 整词边界:不因 "doctor"(含 ct)/"available"(含 lab) 误判为影像/化验
        assert_eq!(
            classify("The doctor saw the patient; results available."),
            DocType::Unknown
        );
    }

    #[test]
    fn classify_surgery() {
        assert_eq!(
            classify("手术记录\n手术经过:行腹腔镜胆囊切除术"),
            DocType::Surgery
        );
    }

    #[test]
    fn classify_clinical_note_beats_incidental_imaging_mention() {
        // 门诊病历正文提到 CT/MRI,不应误判为影像报告
        assert_eq!(
            classify("门诊病历\n就诊科室:神经内科\n复查头颅 MRI 示脑梗死软化灶。"),
            DocType::ClinicalNote
        );
        assert_eq!(
            classify("急诊记录\n急诊头颅 CT 未见出血。"),
            DocType::ClinicalNote
        );
    }

    #[test]
    fn classify_chinese_imaging_keywords() {
        assert_eq!(classify("胸部X线正位片"), DocType::ImagingReport);
        assert_eq!(classify("心电图检查报告"), DocType::ImagingReport);
        assert_eq!(classify("DR 检查:胸部"), DocType::ImagingReport);
        assert_eq!(classify("患者今日拍片复查"), DocType::ImagingReport);
    }

    #[test]
    fn extract_demographics_basic() {
        let d =
            extract_demographics("北京协和医院\n姓名:张建国  性别:男  年龄:60岁 病案号:62198842");
        assert_eq!(d.name.as_deref(), Some("张建国"));
        assert_eq!(d.gender.as_deref(), Some("男"));
        assert_eq!(d.age.as_deref(), Some("60"));
        // 无 demographic 的文本 → 全 None
        let e = extract_demographics("超声所见:肝脏形态正常。");
        assert!(e.name.is_none() && e.gender.is_none() && e.age.is_none());
    }

    #[test]
    fn guess_date_supports_dots() {
        assert_eq!(
            guess_date("报告日期 2023.05.01 完成")
                .unwrap()
                .format("%Y-%m-%d")
                .to_string(),
            "2023-05-01"
        );
    }

    #[test]
    fn guess_date_range_captures_span() {
        // 住院:入院 + 出院 → 区间
        let (s, e) = guess_date_range("入院日期:2023-01-01 出院日期:2023-01-20");
        assert_eq!(s.unwrap().format("%Y-%m-%d").to_string(), "2023-01-01");
        assert_eq!(e.unwrap().format("%Y-%m-%d").to_string(), "2023-01-20");
        // 单点:end 为 None
        let (s1, e1) = guess_date_range("检验日期 2024-07-08");
        assert_eq!(s1.unwrap().format("%Y-%m-%d").to_string(), "2024-07-08");
        assert!(e1.is_none());
        // 无日期
        assert_eq!(guess_date_range("no dates here"), (None, None));
    }

    #[test]
    fn guess_date_prefers_labeled_report_date_over_history() {
        // 复查报告:正文引用既往卒中日期 2023-04-24,但文档日期应取"检查日期"
        let t = "头颅MRI复查\n患者于 2023-04-24 因急性脑梗死入院治疗。\n检查日期:2023-11-02\n所见:软化灶形成。";
        assert_eq!(
            guess_date_range(t)
                .0
                .unwrap()
                .format("%Y-%m-%d")
                .to_string(),
            "2023-11-02"
        );
        // 检测日期优先
        let t2 = "既往 2020-01-01 患糖尿病。\n检测日期:2024-05-18\n肌酐 108";
        assert_eq!(
            guess_date_range(t2)
                .0
                .unwrap()
                .format("%Y-%m-%d")
                .to_string(),
            "2024-05-18"
        );
    }

    #[test]
    fn guess_date_ignores_id_embedded_dates() {
        // 住院号 HS-2024-08-2201 含 "2024-08-22",但两侧紧邻数字 → 不当作日期;
        // 只取真正的出院日期 2024-08-12。
        let t = "住院号:HS-2024-08-2201\n入院日期:2024-08-08 出院日期:2024-08-12";
        let (s, e) = guess_date_range(t);
        assert_eq!(s.unwrap().format("%Y-%m-%d").to_string(), "2024-08-08");
        assert_eq!(e.unwrap().format("%Y-%m-%d").to_string(), "2024-08-12");
    }
}
