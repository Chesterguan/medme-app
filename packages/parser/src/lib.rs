use std::path::Path;
use chrono::{DateTime, TimeZone, Utc};
use core_model::DocType;

pub struct Extracted {
    pub text: String,
    pub page_count: i32,
    pub language: Option<String>,
    pub doc_date: Option<DateTime<Utc>>,
    pub doc_type: DocType,
}

pub fn extract(path: &Path) -> anyhow::Result<Extracted> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let (text, page_count) = match ext.as_str() {
        "txt" => (std::fs::read_to_string(path)?, 1),
        "pdf" => {
            let t = pdf_extract::extract_text(path)?;
            let pages = t.matches('\u{0C}').count() as i32 + 1; // 换页符估页数
            (t, pages)
        }
        other => anyhow::bail!("unsupported extension: {other}"),
    };
    Ok(Extracted {
        language: detect_language(&text),
        doc_date: guess_date(&text),
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

/// Signature shared by the date-parsing helpers below; kept as an alias to
/// avoid a `clippy::type_complexity` warning on the raw `&dyn Fn(...)` form.
type YmdCtor<'a> = &'a dyn Fn(i32, u32, u32) -> Option<DateTime<Utc>>;

pub fn guess_date(text: &str) -> Option<DateTime<Utc>> {
    // 依次尝试:YYYY-MM-DD / YYYY/MM/DD / YYYY年MM月DD日
    let ymd = |y: i32, m: u32, d: u32| Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).single();
    for token in text.split(|c: char| c.is_whitespace()) {
        if let Some(dt) = parse_ymd(token, &ymd) { return Some(dt); }
    }
    // 中文格式全文扫描
    parse_cn_date(text, &ymd)
}

fn parse_ymd(tok: &str, ymd: YmdCtor) -> Option<DateTime<Utc>> {
    let norm = tok.replace('/', "-");
    let parts: Vec<&str> = norm.split('-').collect();
    if parts.len() == 3 {
        if let (Ok(y), Ok(m), Ok(d)) = (parts[0].parse(), parts[1].parse(), parts[2].parse()) {
            if (1900..=2100).contains(&y) { return ymd(y, m, d); }
        }
    }
    None
}

fn parse_cn_date(text: &str, ymd: YmdCtor) -> Option<DateTime<Utc>> {
    let yi = text.find('年')?;
    // 年 之前紧邻的数字 = 年份
    let before: String = text[..yi]
        .chars().rev().take_while(|c| c.is_ascii_digit())
        .collect::<String>().chars().rev().collect();
    let rest = &text[yi + '年'.len_utf8()..];
    let mi = rest.find('月')?;
    let month_end = mi + '月'.len_utf8();
    let di = rest.find('日')?;
    // 月 必须在 日 之前;否则不是合法日期,放弃(checked slice 避免 begin>end panic)
    let day_slice = rest.get(month_end..di)?;
    let m: String = rest[..mi].chars().filter(|c| c.is_ascii_digit()).collect();
    let d: String = day_slice.chars().filter(|c| c.is_ascii_digit()).collect();
    match (before.parse(), m.parse(), d.parse()) {
        (Ok(y), Ok(m), Ok(d)) => ymd(y, m, d),
        _ => None,
    }
}

pub fn classify(text: &str) -> DocType {
    let t = text;
    let has = |kw: &str| t.contains(kw);
    if has("出院记录") || has("discharge") { DocType::DischargeSummary }
    else if has("处方") || has("prescription") { DocType::Prescription }
    else if has("检验") || has("化验") || has("lab") { DocType::LabReport }
    else if has("影像") || has("CT") || has("MRI") || has("超声") { DocType::ImagingReport }
    else if has("病理") || has("pathology") { DocType::Pathology }
    else { DocType::Unknown }
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
        assert_eq!(e.doc_date.unwrap().format("%Y-%m-%d").to_string(), "2023-05-01");
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
}
