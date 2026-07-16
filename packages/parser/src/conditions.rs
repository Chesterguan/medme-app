//! Deterministic diagnosis extraction (stage B).
//!
//! Pulls diagnosis terms out of labeled sections of a clinical note and keeps
//! them as honest raw strings. Pure string work: no network, no LLM. There is
//! **no** condition/diagnosis category in the terminology dictionary, so — unlike
//! labs and meds — nothing here is normalized to a code (ICD or otherwise). Any
//! ICD code that happens to be printed alongside a term is *dropped*, not
//! trusted: we don't verify it and won't launder it into structured data.
//!
//! ## Row shapes handled
//! Section label (optionally after a list number) + `:`/`：`, then either:
//! ```text
//! 出院诊断:2型糖尿病；高血压病3级          <- inline, split on ；;，,、 and numbers
//! 出院诊断:1. 急性脑梗死 2. 高血压3级 3. 2型糖尿病  <- inline w/ in-line numbering
//! 出院诊断:                                <- label then a numbered block:
//!   1. 2型糖尿病(E11.9)                    <- numbering stripped, ICD code dropped
//!   2. 高血压病3级
//! ```
//! Recognized labels: 诊断 初步诊断 入院诊断 出院诊断 主要诊断 其他诊断 临床诊断.
//! A numbered block is consumed until a blank line or a non-numbered line.
//!
//! ## 病理诊断 is deliberately NOT a diagnosis label here
//! 病理 reports write a *narrative* impression (`(胃窦)慢性活动性胃炎,伴轻度肠上皮
//! 化生,Hp 阳性(++)。未见异型增生及恶性证据。`) that must never be comma-split into
//! fake "diagnoses" — it is surfaced as a pathology **conclusion** by the summary
//! layer (docs/030, quality dim 6), not as problems.
//!
//! ## Deliberately NOT handled (kept lean)
//! - Diagnoses in free prose with no section label.
//! - Negation / history qualifiers (`否认…`, `既往…`) — the term is kept verbatim.
//! - Splitting one term into disease + stage/laterality (`高血压病3级` stays whole).
//! - Any normalization / de-duplication across synonyms — only exact
//!   (raw_text, section) duplicates are collapsed.

use regex::Regex;
use std::collections::HashSet;
use std::sync::OnceLock;

/// One diagnosis term as written, tagged with the section label it came under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionMention {
    pub raw_text: String,
    pub section: Option<String>,
}

/// A diagnosis-section label line: optional list marker, a known label, a colon,
/// then the (possibly empty) inline remainder. Longer labels precede `诊断` so a
/// specific section name wins over the generic one.
fn section_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"^\s*(?:\d+\s*[.、)）]|[\u{2460}-\u{2473}]|[-•*·])?\s*(出院诊断|入院诊断|初步诊断|主要诊断|其他诊断|临床诊断|诊断)\s*[:：]\s*(.*)$",
        )
        .expect("section re")
    })
}

/// A numbered list item: `1. xxx` / `2、xxx` / `①xxx`. Captures the content after
/// the marker. A delimiter after the digits is required, so a bare `2型糖尿病`
/// (no delimiter) is NOT mistaken for a numbered item.
fn numbered_item_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"^\s*(?:\d+\s*[.、)）]|[\u{2460}-\u{2473}])\s*(.+)$").expect("numbered item re")
    })
}

/// Trailing ICD-style code in (), （）, [], 【】: `(E11.9)`, `[I10]`. Inner content
/// must start with a letter+digit — the ICD-10 shape — so a genuine parenthetical
/// like `高血压(3级)` is left intact.
fn icd_paren_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"\s*[(（\[【]\s*[A-Za-z]\d[0-9A-Za-z.\-]*\s*[)）\]】]\s*$").expect("icd re")
    })
}

/// An **in-line** numbered marker: whitespace (or line start) then `N.`/`N、`/`N)`.
/// The delimiter after the digit is required, so `高血压 3 级` / `2 型糖尿病` (a digit
/// glued to the disease name, no delimiter) is NOT treated as a marker.
///
/// 真 corpus 把多诊断写在一行:`出院诊断:1. 急性脑梗死 2. 高血压3级 3. 2型糖尿病`。
/// 这些 ` 2.` ` 3.` 既不是标点分隔符也不是「独立成行」的编号块,过去整行塌成一条
/// term。先按它切开,行内多诊断才拆得开(quality dim 1)。
fn inline_number_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // Whitespace (or end) MUST follow the delimiter, so a decimal measurement inside
    // a diagnosis (`甲状腺结节 1.2cm`) is not split at the `.` — the corpus always
    // writes list markers as `1. ` / `2. ` with a trailing space.
    R.get_or_init(|| Regex::new(r"(?:^|\s)\d+\s*[.、)）](?:\s+|$)").expect("inline number re"))
}

/// Split an inline diagnosis string, clean each part, keep order. Two passes:
/// first on in-line numbered markers (` 2.` ` 3.`), then on `；;，,、`.
fn split_inline(s: &str) -> Vec<String> {
    inline_number_re()
        .split(s)
        .flat_map(|seg| seg.split(['；', ';', '，', ',', '、']))
        .filter_map(clean_dx)
        .collect()
}

/// Normalize one diagnosis term: strip any leading list numbering, drop a
/// trailing ICD code, trim punctuation/space. Returns `None` for empties.
fn clean_dx(raw: &str) -> Option<String> {
    let mut s = numbered_item_re()
        .captures(raw)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
        .unwrap_or_else(|| raw.to_string());
    s = icd_paren_re().replace(&s, "").to_string();
    let s = s
        .trim()
        .trim_matches(|c: char| {
            c.is_whitespace()
                || matches!(c, '.' | '。' | '、' | '，' | ',' | ';' | '；' | ':' | '：')
        })
        .to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Extract diagnosis mentions from labeled sections. De-dups identical
/// (raw_text, section) pairs; keeps raw strings (no terminology normalization).
pub fn extract_conditions(text: &str) -> Vec<ConditionMention> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(caps) = section_re().captures(lines[i]) else {
            i += 1;
            continue;
        };
        let section = caps.get(1).expect("label group").as_str().to_string();
        let inline = caps.get(2).map(|m| m.as_str()).unwrap_or("");

        let mut push = |dx: String, out: &mut Vec<ConditionMention>| {
            if seen.insert((dx.clone(), section.clone())) {
                out.push(ConditionMention {
                    raw_text: dx,
                    section: Some(section.clone()),
                });
            }
        };

        // Inline diagnoses after the label.
        for dx in split_inline(inline) {
            push(dx, &mut out);
        }

        // A following numbered block belongs to this section; stop at a blank or
        // non-numbered line.
        let mut j = i + 1;
        while j < lines.len() {
            if lines[j].trim().is_empty() || !numbered_item_re().is_match(lines[j]) {
                break;
            }
            if let Some(dx) = clean_dx(lines[j]) {
                push(dx, &mut out);
            }
            j += 1;
        }
        i = j.max(i + 1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_split_and_section_captured() {
        let obs = extract_conditions("出院诊断:2型糖尿病；高血压病3级");
        assert_eq!(obs.len(), 2);
        assert_eq!(obs[0].raw_text, "2型糖尿病");
        assert_eq!(obs[0].section.as_deref(), Some("出院诊断"));
        assert_eq!(obs[1].raw_text, "高血压病3级");
        assert_eq!(obs[1].section.as_deref(), Some("出院诊断"));
    }

    #[test]
    fn inline_numbered_multi_diagnosis_splits() {
        // 真 corpus 出院诊断:一行内用 `1. .. 2. .. 3. ..` 串三个诊断,须拆成三条,
        // 且 term 里不含行内编号标记(`2.`/`3.`),病名内的空格保留(逐字)。
        let obs = extract_conditions("出院诊断:1. 急性脑梗死  2. 高血压 3 级(很高危)  3. 2 型糖尿病");
        let terms: Vec<&str> = obs.iter().map(|o| o.raw_text.as_str()).collect();
        assert_eq!(terms, ["急性脑梗死", "高血压 3 级(很高危)", "2 型糖尿病"]);
        assert!(obs.iter().all(|o| !o.raw_text.contains(" 2.")
            && !o.raw_text.contains(" 3.")
            && !o.raw_text.contains('。')));
    }

    #[test]
    fn decimal_measurement_in_diagnosis_is_not_split() {
        // 病名带尺寸的小数(`1.2cm`)不得被行内编号切分成 `["甲状腺结节","2cm"]`。
        let obs = extract_conditions("诊断:甲状腺结节 1.2cm");
        let terms: Vec<&str> = obs.iter().map(|o| o.raw_text.as_str()).collect();
        assert_eq!(terms, ["甲状腺结节 1.2cm"]);
    }

    #[test]
    fn numbered_block_and_icd_code_stripped() {
        let text = "\
出院诊断:
1. 2型糖尿病(E11.9)
2. 高血压病3级

医师签名:王五
";
        let obs = extract_conditions(text);
        assert_eq!(obs.len(), 2);
        // ICD code dropped, disease-leading digit (2型) preserved.
        assert_eq!(obs[0].raw_text, "2型糖尿病");
        assert_eq!(obs[0].section.as_deref(), Some("出院诊断"));
        assert_eq!(obs[1].raw_text, "高血压病3级");
    }

    #[test]
    fn label_variants_and_dedup() {
        // A different label is captured; duplicates within a section collapse.
        let text = "\
初步诊断：冠心病、冠心病
其他诊断:高尿酸血症
";
        let obs = extract_conditions(text);
        assert_eq!(obs.len(), 2);
        assert_eq!(obs[0].raw_text, "冠心病");
        assert_eq!(obs[0].section.as_deref(), Some("初步诊断"));
        assert_eq!(obs[1].raw_text, "高尿酸血症");
        assert_eq!(obs[1].section.as_deref(), Some("其他诊断"));
    }

    #[test]
    fn bracket_icd_and_non_diagnosis_lines_ignored() {
        let text = "\
主诉:多饮多尿1年
临床诊断:2型糖尿病[E11.9]
血压:130/80
";
        let obs = extract_conditions(text);
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].raw_text, "2型糖尿病");
        assert_eq!(obs[0].section.as_deref(), Some("临床诊断"));
    }
}
