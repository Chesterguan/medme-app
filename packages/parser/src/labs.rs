//! Deterministic lab-value extraction (stage B).
//!
//! Turns a lab report's OCR text into structured, normalized [`LabObservation`]
//! rows. Pure string work: no network, no LLM. Normalization/coding is delegated
//! to the `terminology` crate — this module only locates rows, parses numbers,
//! and asks terminology what each analyte is.
//!
//! ## Row shapes handled
//! Chinese lab reports are table-ish; columns are separated by 2+ spaces or tabs:
//! ```text
//! 项目            结果    单位        参考范围     [↑/↓]
//! 肌酐            88      μmol/L      59-104
//! 谷丙转氨酶(ALT) 45      U/L         0-40         ↑
//! 低密度脂蛋白胆固醇 3.6   mmol/L      <3.4         ↑
//! ```
//! Plus the labeled inline form: `肌酐: 88 μmol/L (参考 59-104)`.
//!
//! A line is treated as a lab row when it has a name token (contains a letter or
//! CJK char) + a numeric result AND at least one piece of lab evidence (a unit, a
//! reference range, an explicit H/L marker, or a successful terminology match).
//! That evidence gate is what skips demographics like `年龄:60` without a
//! blocklist — honest, deterministic, and it still keeps genuine-but-unknown
//! analytes (they carry a unit/range).
//!
//! ## Deliberately NOT handled (kept lean)
//! - Ratio-style results printed as one token (`血压 120/80`) — the `/80` is
//!   mistaken for a unit; blood pressure is a vital, out of scope here.
//! - Results whose value is glued to the name with no separator (`肌酐88`).
//! - Multi-line wrapped rows (name on one line, value on the next).
//! - Reference ranges are parsed/stored in the RAW reporting unit only; the
//!   struct has no canonical-ref fields, so refs are compared against the raw
//!   value (same unit) for flagging and left un-converted.

use regex::Regex;
use std::sync::OnceLock;
use terminology::{dictionary_entries, normalize_unit, resolve};

/// One normalized lab result row. Mapping is additive: the raw name/value is
/// always kept even when terminology can't resolve it (upper layer decides).
#[derive(Debug, Clone)]
pub struct LabObservation {
    pub raw_name: String,
    pub analyte_key: Option<String>,
    pub canonical_name: Option<String>,
    pub loinc: Option<String>,
    pub value_num: f64,
    pub value_canonical: Option<f64>,
    pub unit_raw: Option<String>,
    pub unit_canonical: Option<String>,
    pub ref_low: Option<f64>,
    pub ref_high: Option<f64>,
    /// "H" | "L" | "N": explicit ↑/↓/H/L marker if present, else value-vs-ref.
    pub flag: Option<String>,
    /// 0.0 if unmatched; else the terminology `Match.confidence`.
    pub confidence: f32,
}

/// `name  value  rest`. Name is non-greedy so `value` is the FIRST number after
/// the first separator run (space/tab/colon) — i.e. the result column.
fn row_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"^\s*(?P<name>.*?)[\s:：]+(?P<value>-?\d+(?:\.\d+)?)\s*(?P<rest>.*)$")
            .expect("row re")
    })
}

fn range_two_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^(\d+(?:\.\d+)?)[-~](\d+(?:\.\d+)?)$").expect("range re"))
}
fn range_high_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^[<≤]=?(\d+(?:\.\d+)?)$").expect("high re"))
}
fn range_low_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^[>≥]=?(\d+(?:\.\d+)?)$").expect("low re"))
}

/// Fold the full-width comparison/range punctuation a report might use into the
/// half-width forms the range regexes expect. Pure notation, not semantics.
fn fold_range_punct(tok: &str) -> String {
    tok.chars()
        .map(|c| match c {
            '～' => '~',
            '－' | '—' | '−' => '-',
            '＜' => '<',
            '＞' => '>',
            _ => c,
        })
        .collect()
}

/// Parse one token as a reference range. Returns `(low, high)`:
/// `59-104` → (Some, Some); `<6.5`/`≤6.5` → (None, Some); `>130`/`≥130` → (Some, None).
/// Returns `None` when the token is not a range at all.
fn parse_range(tok: &str) -> Option<(Option<f64>, Option<f64>)> {
    let s = fold_range_punct(tok);
    if let Some(c) = range_two_re().captures(&s) {
        let lo = c.get(1)?.as_str().parse().ok()?;
        let hi = c.get(2)?.as_str().parse().ok()?;
        return Some((Some(lo), Some(hi)));
    }
    if let Some(c) = range_high_re().captures(&s) {
        return Some((None, Some(c.get(1)?.as_str().parse().ok()?)));
    }
    if let Some(c) = range_low_re().captures(&s) {
        return Some((Some(c.get(1)?.as_str().parse().ok()?), None));
    }
    None
}

/// Parse the trailing `单位 参考范围 [↑/↓]` columns. Order-independent: every
/// whitespace token is classified as flag / range / unit / (ignored label).
fn parse_rest(rest: &str) -> (Option<String>, Option<f64>, Option<f64>, Option<String>) {
    let (mut unit, mut low, mut high, mut flag) = (None, None, None, None);
    for raw in rest.split_whitespace() {
        let tok =
            raw.trim_matches(|c| matches!(c, '(' | ')' | '（' | '）' | '[' | ']' | '【' | '】'));
        if tok.is_empty() {
            continue;
        }
        // Explicit flag markers (arrows may be glued to another token).
        if raw.contains('↑') || tok == "H" || tok == "高" || tok == "偏高" {
            flag = Some("H".to_string());
            continue;
        }
        if raw.contains('↓') || tok == "L" || tok == "低" || tok == "偏低" {
            flag = Some("L".to_string());
            continue;
        }
        if let Some((lo, hi)) = parse_range(tok) {
            if lo.is_some() {
                low = lo;
            }
            if hi.is_some() {
                high = hi;
            }
            continue;
        }
        // Label noise inside inline `(参考 …)` / `正常范围` etc.
        if tok.contains("参考") || tok.contains("范围") || tok.contains("正常") {
            continue;
        }
        // First unit-looking token wins (has a letter, %, / or degree sign).
        if unit.is_none()
            && tok
                .chars()
                .any(|c| c.is_ascii_alphabetic() || c == '%' || c == '/' || c == '°')
        {
            unit = Some(tok.to_string());
        }
    }
    (unit, low, high, flag)
}

/// Extract normalized lab observations from a report's text. Unknown analytes
/// are kept (analyte_key = None, confidence 0.0), never dropped.
pub fn extract_labs(text: &str) -> Vec<LabObservation> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(caps) = row_re().captures(line) else {
            continue;
        };
        let raw_name = caps.name("name").expect("name group").as_str().trim();
        // Need a real name token — rejects date/number-only lines.
        if raw_name.is_empty() || !raw_name.chars().any(|c| c.is_alphabetic()) {
            continue;
        }
        let Ok(value_num) = caps
            .name("value")
            .expect("value group")
            .as_str()
            .parse::<f64>()
        else {
            continue;
        };
        let rest = caps.name("rest").expect("rest group").as_str();
        let (unit_raw, ref_low, ref_high, explicit_flag) = parse_rest(rest);

        let m = resolve(raw_name, unit_raw.as_deref());
        // Lab-row gate: some evidence beyond "a name and a number" must exist,
        // else it's demographics/metadata (年龄:60) — skip it.
        let has_evidence = unit_raw.is_some()
            || ref_low.is_some()
            || ref_high.is_some()
            || explicit_flag.is_some()
            || m.is_some();
        if !has_evidence {
            continue;
        }

        // Canonical conversion (only when matched AND the entry knows this unit).
        let mut value_canonical = None;
        let mut unit_canonical = None;
        if let (Some(m), Some(u)) = (&m, &unit_raw) {
            if let Some(entry) = dictionary_entries().iter().find(|e| e.key == m.key) {
                let nu = normalize_unit(u);
                if let Some(conv) = entry.units.iter().find(|c| normalize_unit(&c.unit) == nu) {
                    value_canonical = Some(conv.slope * value_num + conv.intercept);
                    unit_canonical = entry.canonical_unit.clone();
                }
            }
        }

        // Flag: explicit marker wins; else compare raw value against raw refs.
        let flag = explicit_flag.or_else(|| {
            if ref_high.is_some_and(|h| value_num > h) {
                Some("H".to_string())
            } else if ref_low.is_some_and(|l| value_num < l) {
                Some("L".to_string())
            } else if ref_low.is_some() || ref_high.is_some() {
                Some("N".to_string())
            } else {
                None
            }
        });

        out.push(LabObservation {
            raw_name: raw_name.to_string(),
            analyte_key: m.as_ref().map(|m| m.key.clone()),
            canonical_name: m.as_ref().map(|m| m.canonical_name.clone()),
            loinc: m.as_ref().and_then(|m| m.codes.loinc.clone()),
            value_num,
            value_canonical,
            unit_raw,
            unit_canonical,
            ref_low,
            ref_high,
            flag,
            confidence: m.as_ref().map_or(0.0, |m| m.confidence),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find<'a>(obs: &'a [LabObservation], key: &str) -> &'a LabObservation {
        obs.iter()
            .find(|o| o.analyte_key.as_deref() == Some(key))
            .unwrap_or_else(|| panic!("no observation for {key}"))
    }

    #[test]
    fn renal_panel_extracts_creatinine() {
        let text = "\
生化检验报告单
项目            结果    单位        参考范围
肌酐            88      μmol/L      59-104
尿素            5.2     mmol/L      2.9-8.2
尿酸            380     μmol/L      150-420
";
        let obs = extract_labs(text);
        let cr = find(&obs, "creatinine");
        assert_eq!(cr.value_num, 88.0);
        assert_eq!(cr.raw_name, "肌酐");
        assert!(cr.loinc.is_some(), "creatinine must carry a LOINC");
        assert_eq!(cr.unit_canonical.as_deref(), Some("umol/L"));
        assert_eq!(cr.value_canonical, Some(88.0)); // identity conversion
        assert_eq!(cr.flag.as_deref(), Some("N")); // 88 within 59-104
        assert_eq!(cr.confidence, 1.0);
    }

    #[test]
    fn lipids_ldl_flag_high() {
        let text = "\
血脂四项
低密度脂蛋白胆固醇  3.6  mmol/L  <3.4  ↑
";
        let obs = extract_labs(text);
        let ldl = find(&obs, "ldl");
        assert_eq!(ldl.value_num, 3.6);
        assert_eq!(ldl.ref_high, Some(3.4));
        assert_eq!(ldl.ref_low, None);
        assert_eq!(ldl.flag.as_deref(), Some("H"));
    }

    #[test]
    fn cbc_hemoglobin_flag_low() {
        let text = "\
血常规
血红蛋白    109     g/L       130-175   ↓
";
        let obs = extract_labs(text);
        let hb = find(&obs, "hgb");
        assert_eq!(hb.value_num, 109.0);
        assert_eq!(hb.unit_canonical.as_deref(), Some("g/L"));
        assert_eq!(hb.value_canonical, Some(109.0));
        assert_eq!(hb.flag.as_deref(), Some("L"));
    }

    #[test]
    fn mgdl_value_converts_to_canonical() {
        // Inline labeled form + mg/dL that must convert: 1.2 mg/dL * 88.42 ≈ 106.1 µmol/L.
        let text = "肌酐: 1.2 mg/dL (参考 0.6-1.3)";
        let obs = extract_labs(text);
        let cr = find(&obs, "creatinine");
        assert_eq!(cr.unit_raw.as_deref(), Some("mg/dL"));
        assert_eq!(cr.unit_canonical.as_deref(), Some("umol/L"));
        let vc = cr.value_canonical.expect("mg/dL must convert");
        assert!((vc - 106.104).abs() < 0.01, "got {vc}");
    }

    #[test]
    fn unmatched_analyte_row_is_kept() {
        let text = "神秘指标XYZ   12.3   mg/L   0-5";
        let obs = extract_labs(text);
        assert_eq!(obs.len(), 1);
        let o = &obs[0];
        assert_eq!(o.analyte_key, None);
        assert_eq!(o.canonical_name, None);
        assert_eq!(o.loinc, None);
        assert_eq!(o.confidence, 0.0);
        assert_eq!(o.value_num, 12.3);
        assert_eq!(o.unit_raw.as_deref(), Some("mg/L"));
        assert_eq!(o.flag.as_deref(), Some("H")); // 12.3 > 5, computed from ref
    }

    #[test]
    fn header_and_section_lines_are_skipped() {
        let text = "\
生化检验报告单
姓名:张三  性别:男  年龄:60
项目            结果    单位        参考范围
谷丙转氨酶(ALT)  45     U/L         0-40    ↑
空腹血糖        6.9     mmol/L      3.9-6.1
";
        let obs = extract_labs(text);
        // Only the two real data rows survive; header/section/demographics gone.
        assert_eq!(obs.len(), 2, "got {:?}", obs);
        assert!(obs.iter().all(|o| o.raw_name != "项目"));
        assert!(obs.iter().all(|o| o.raw_name != "年龄"));
        let alt = find(&obs, "alt");
        assert_eq!(alt.value_num, 45.0);
        assert_eq!(alt.flag.as_deref(), Some("H")); // explicit ↑
        let glu = find(&obs, "glucose");
        assert_eq!(glu.value_num, 6.9);
        assert_eq!(glu.flag.as_deref(), Some("H")); // 6.9 > 6.1, computed
    }
}
