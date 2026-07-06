use chrono::{DateTime, TimeZone, Utc};
use core_model::DocType;
use regex::Regex;
use std::path::Path;
use std::sync::OnceLock;

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
            let t = pdf_extract::extract_text(path)?;
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
pub fn guess_date_range(text: &str) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    static ISO: OnceLock<Regex> = OnceLock::new();
    static CN: OnceLock<Regex> = OnceLock::new();
    let iso = ISO.get_or_init(|| {
        Regex::new(r"(\d{4})[-/.](\d{1,2})[-/.](\d{1,2})").expect("static ISO date regex compiles")
    });
    let cn = CN.get_or_init(|| {
        Regex::new(r"(\d{4})\s*年\s*(\d{1,2})\s*月\s*(\d{1,2})\s*日")
            .expect("static CN date regex compiles")
    });
    let mut dates: Vec<DateTime<Utc>> = Vec::new();
    for caps in iso.captures_iter(text) {
        if let Some(d) = build_date(&caps) {
            dates.push(d);
        }
    }
    for caps in cn.captures_iter(text) {
        if let Some(d) = build_date(&caps) {
            dates.push(d);
        }
    }
    dates.sort();
    dates.dedup();
    match dates.len() {
        0 => (None, None),
        1 => (Some(dates[0]), None),
        _ => (Some(dates[0]), Some(dates[dates.len() - 1])),
    }
}

/// 单点日期(向后兼容):取最早。
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

pub fn classify(text: &str) -> DocType {
    let lower = text.to_lowercase(); // 拉丁字母小写化;中文不变,仍能匹配
    let has = |kw: &str| lower.contains(kw);
    // 短英文缩写用整词匹配,避免 "doctor"(含 ct)/"available"(含 lab)误命中
    let words: std::collections::HashSet<&str> =
        lower.split(|c: char| !c.is_alphanumeric()).filter(|s| !s.is_empty()).collect();
    let word = |w: &str| words.contains(w);

    if has("出院记录") || has("discharge") {
        DocType::DischargeSummary
    } else if has("处方") || has("prescription") {
        DocType::Prescription
    } else if has("检验") || has("化验") || has("laborator") || word("lab") {
        DocType::LabReport
    } else if has("影像") || has("超声") || has("ultrasound") || has("imaging")
        || has("radiolog") || has("computed tomograph") || has("magnetic resonance")
        || word("ct") || word("mri") || word("xray") || has("x-ray") {
        DocType::ImagingReport
    } else if has("病理") || has("pathology") {
        DocType::Pathology
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
        assert_eq!(guess_date("出院日期:2023-05-01    科室:神经内科")
            .unwrap().format("%Y-%m-%d").to_string(), "2023-05-01");
        // 文本含"年龄"(含"年")时,中文日期仍需正确解析
        let t = "姓名:张三 年龄:60岁 检查日期:2025年02月18日 影像所见";
        assert_eq!(guess_date(t).unwrap().format("%Y-%m-%d").to_string(), "2025-02-18");
        // 斜杠格式,带时间后缀
        assert_eq!(guess_date("采集 2024/01/15 07:52")
            .unwrap().format("%Y-%m-%d").to_string(), "2024-01-15");
        // 空占位符 → 无有效日期
        assert!(guess_date("检测日期:____年__月__日").is_none());
    }

    #[test]
    fn classify_case_insensitive_and_english() {
        assert_eq!(classify("Discharge Summary\nDiagnosis: pneumonia"), DocType::DischargeSummary);
        assert_eq!(classify("Laboratory Report\nHemoglobin 140"), DocType::LabReport);
        assert_eq!(classify("Ultrasound Report\nfatty liver"), DocType::ImagingReport);
        assert_eq!(classify("chest CT scan report\nnodule"), DocType::ImagingReport);
        // 整词边界:不因 "doctor"(含 ct)/"available"(含 lab) 误判为影像/化验
        assert_eq!(classify("The doctor saw the patient; results available."), DocType::Unknown);
    }

    #[test]
    fn guess_date_supports_dots() {
        assert_eq!(guess_date("报告日期 2023.05.01 完成").unwrap().format("%Y-%m-%d").to_string(), "2023-05-01");
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
}
