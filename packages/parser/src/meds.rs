//! Deterministic medication extraction (stage B).
//!
//! Turns a prescription / medication line into a structured [`MedObservation`]:
//! drug name (normalized via `terminology::resolve_drug`), dose (number + unit)
//! and a normalized frequency code. Pure string work: no network, no LLM. Coding
//! is delegated to the `terminology` crate — this module only locates the parts
//! of a line and asks terminology what the drug is. Unmatched-but-clearly-a-med
//! lines are kept (drug_key = None, confidence 0.0); we never invent codes.
//!
//! ## Row shapes handled
//! One medication per line, name-first, with dose / frequency / route trailing:
//! ```text
//! 二甲双胍 0.5g bid
//! 阿托伐他汀钙片 20mg 每晚一次
//! 1. 盐酸二甲双胍片 0.5g 每日两次口服
//! 氨氯地平  5mg  qd  po
//! 胰岛素 12U 三餐前
//! ```
//! - Leading list markers (`1.` `1、` `①` `-` `•`) are stripped.
//! - Dose is the first `number+unit` token; units: g, mg, μg/ug/mcg, IU/U,
//!   mL/ml, 片/粒/袋/支/丸/滴/单位. Glued (`20mg`) or spaced (`20 mg`).
//! - Frequency maps English abbrevs AND Chinese to one code (qd/bid/tid/qid/qn/
//!   q8h/q12h/prn/qw/"tid ac"); the original substring is kept in `frequency_raw`.
//! - Trailing route words (口服/po/静滴/iv/皮下/sc …) are stripped off the name.
//!
//! ## Deliberately NOT handled (kept lean)
//! - Multiple drugs on one line — one line, one drug.
//! - Compound / tapering schedules (`早1片晚2片`, `逐渐减量`), ranges (`1-2片`).
//! - Duration / total-quantity / dispense counts (`共14天`, `×7`, `14盒`).
//! - Dose written without a recognized unit, or glued to the name with no
//!   separator; a name with neither a dose, a frequency, nor a drug match is not
//!   treated as a medication line (skipped, not invented).

use regex::Regex;
use std::sync::OnceLock;
use terminology::resolve_drug;

/// One parsed medication line. Mapping is additive: the raw drug name is always
/// kept even when terminology can't resolve it (upper layer decides trust).
#[derive(Debug, Clone)]
pub struct MedObservation {
    pub raw_name: String,
    pub drug_key: Option<String>,
    pub canonical_name: Option<String>,
    pub ingredient: Option<String>,
    pub rxnorm: Option<String>,
    pub atc: Option<String>,
    pub dose_num: Option<f64>,
    pub dose_unit: Option<String>,
    /// Normalized code: "qd"|"bid"|"tid"|"qid"|"qn"|"q8h"|"q12h"|"prn"|"qw"|"tid ac".
    pub frequency: Option<String>,
    /// The original frequency substring as written (e.g. "每日两次").
    pub frequency_raw: Option<String>,
    /// 0.0 if unmatched; else the terminology `Match.confidence`.
    pub confidence: f32,
}

/// Leading list marker: `1.` `1、` `1)` `①`..`⑩` `-` `•` `*` `·`.
fn list_marker_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"^\s*(?:\d+\s*[.、)）]|[\u{2460}-\u{2473}]|[-•*·])\s*").expect("list marker re")
    })
}

/// First `number+unit` dose token. Multi-char units precede single-char ones so
/// `20mg` matches `mg` (not `g`) and `12iu` matches `iu` (not `u`).
fn dose_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)(\d+(?:\.\d+)?)\s*(µg|μg|mcg|mg|iu|ml|ug|单位|g|u|片|粒|袋|支|丸|滴)")
            .expect("dose re")
    })
}

/// Frequency patterns in priority order: more specific first so `每晚一次` maps
/// to `qn` (not `qd` via `一次`) and `三餐前` to `tid ac` (not `tid`). Each entry
/// is `(code, pattern)`; the first pattern that matches the line wins.
fn freq_patterns() -> &'static [(&'static str, Regex)] {
    static R: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    R.get_or_init(|| {
        let raw: &[(&str, &str)] = &[
            ("qn", r"(?i)每晚一次|每晚|睡前|\bqn\b"),
            ("tid ac", r"三餐前|餐前"),
            ("qw", r"(?i)每周一次|每周1次|\bqw\b"),
            ("q12h", r"(?i)每12小时|\bq12h\b"),
            ("q8h", r"(?i)每8小时|\bq8h\b"),
            (
                "bid",
                r"(?i)每日两次|每日2次|一日两次|一日2次|每天两次|\bbid\b",
            ),
            ("tid", r"(?i)每日三次|一日三次|每天三次|\btid\b"),
            ("qid", r"(?i)每日四次|一日四次|每天四次|\bqid\b"),
            ("qd", r"(?i)每日一次|一日一次|每天一次|每日1次|\bqd\b"),
            ("prn", r"(?i)必要时|按需|\bprn\b"),
        ];
        raw.iter()
            .map(|(code, pat)| (*code, Regex::new(pat).expect("freq re")))
            .collect()
    })
}

/// Trailing administration-route words to peel off the name. `qn`/`睡前` etc. are
/// frequencies, not routes, and are handled elsewhere.
const ROUTE_WORDS: &[&str] = &[
    "口服",
    "po",
    "静滴",
    "静脉滴注",
    "静脉注射",
    "静推",
    "iv",
    "皮下",
    "sc",
    "肌注",
    "im",
    "外用",
    "含服",
    "舌下",
    "鼻饲",
];

/// Canonicalize a matched dose unit to a stable spelling. CJK units pass through.
fn normalize_dose_unit(u: &str) -> String {
    match u.to_lowercase().as_str() {
        "mcg" | "ug" | "µg" | "μg" => "µg".to_string(),
        "iu" => "IU".to_string(),
        "u" => "U".to_string(),
        "ml" => "mL".to_string(),
        "mg" => "mg".to_string(),
        "g" => "g".to_string(),
        _ => u.to_string(),
    }
}

/// Parse the first dose token: `(number, canonical_unit, byte_start_of_token)`.
fn parse_dose(line: &str) -> Option<(f64, String, usize)> {
    let caps = dose_re().captures(line)?;
    let whole = caps.get(0)?;
    let num: f64 = caps.get(1)?.as_str().parse().ok()?;
    let unit = normalize_dose_unit(caps.get(2)?.as_str());
    Some((num, unit, whole.start()))
}

/// Parse the frequency: `(code, raw_substring, byte_start)`. First priority-order
/// pattern that matches wins (see [`freq_patterns`]).
fn parse_frequency(line: &str) -> Option<(String, String, usize)> {
    for (code, re) in freq_patterns() {
        if let Some(m) = re.find(line) {
            return Some((code.to_string(), m.as_str().to_string(), m.start()));
        }
    }
    None
}

/// Strip trailing route words and dangling punctuation/space off the name token.
fn strip_trailing_route(mut name: &str) -> &str {
    loop {
        let trimmed = name.trim_end_matches([' ', '\t', ',', '，', '、', ':', '：', '(', '（']);
        let mut cut = trimmed;
        for r in ROUTE_WORDS {
            // Case-insensitive suffix match. The boundary check guards ASCII
            // route words (`po`) from slicing into a preceding CJK char.
            let start = trimmed.len().wrapping_sub(r.len());
            if trimmed.len() >= r.len()
                && trimmed.is_char_boundary(start)
                && trimmed[start..].eq_ignore_ascii_case(r)
            {
                cut = &trimmed[..start];
                break;
            }
        }
        if cut.len() == name.len() {
            return name.trim();
        }
        name = cut;
    }
}

/// Extract medication observations, one per line. Unmatched-but-clearly-a-med
/// lines (they carry a dose or a frequency) are kept with drug_key = None.
pub fn extract_meds(text: &str) -> Vec<MedObservation> {
    let mut out = Vec::new();
    for line in text.lines() {
        // 1) strip leading list marker.
        let cleaned = list_marker_re().replace(line, "");
        let cleaned = cleaned.trim();
        if cleaned.is_empty() {
            continue;
        }

        // 2) locate dose + frequency.
        let dose = parse_dose(cleaned);
        let freq = parse_frequency(cleaned);

        // 3) name = everything before the earliest of dose/frequency; strip route.
        let name_end = match (dose.as_ref().map(|d| d.2), freq.as_ref().map(|f| f.2)) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => cleaned.len(),
        };
        let name = strip_trailing_route(&cleaned[..name_end]);
        // Need a real name token (a letter or CJK char), else it's not a med line.
        if name.is_empty() || !name.chars().any(|c| c.is_alphabetic()) {
            continue;
        }

        // 4) resolve the drug (prescription namespace).
        let m = resolve_drug(name);

        // Med-line gate: some evidence beyond a bare name — a dose, a frequency,
        // or a drug match — else it's prose that merely mentions a word.
        if dose.is_none() && freq.is_none() && m.is_none() {
            continue;
        }

        out.push(MedObservation {
            raw_name: name.to_string(),
            drug_key: m.as_ref().map(|m| m.key.clone()),
            canonical_name: m.as_ref().map(|m| m.canonical_name.clone()),
            ingredient: m.as_ref().and_then(|m| m.ingredient.clone()),
            rxnorm: m.as_ref().and_then(|m| m.codes.rxnorm.clone()),
            atc: m.as_ref().and_then(|m| m.codes.atc.clone()),
            dose_num: dose.as_ref().map(|d| d.0),
            dose_unit: dose.as_ref().map(|d| d.1.clone()),
            frequency: freq.as_ref().map(|f| f.0.clone()),
            frequency_raw: freq.as_ref().map(|f| f.1.clone()),
            confidence: m.as_ref().map_or(0.0, |m| m.confidence),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dose_and_freq_glued_and_spaced() {
        // Glued 0.5g + English bid.
        let o = &extract_meds("二甲双胍 0.5g bid")[0];
        assert_eq!(o.drug_key.as_deref(), Some("metformin"));
        assert_eq!(o.dose_num, Some(0.5));
        assert_eq!(o.dose_unit.as_deref(), Some("g"));
        assert_eq!(o.frequency.as_deref(), Some("bid"));
        assert_eq!(o.rxnorm.as_deref(), Some("6809"));
        assert!(o.confidence >= 0.8);

        // Spaced 20 mg + English qd + trailing route po.
        let o = &extract_meds("氨氯地平  20 mg  qd  po")[0];
        assert_eq!(o.raw_name, "氨氯地平");
        assert_eq!(o.drug_key.as_deref(), Some("amlodipine"));
        assert_eq!(o.dose_num, Some(20.0));
        assert_eq!(o.dose_unit.as_deref(), Some("mg"));
        assert_eq!(o.frequency.as_deref(), Some("qd"));
    }

    #[test]
    fn chinese_frequencies_normalize() {
        // 每晚一次 -> qn (not qd), and dose still parses.
        let o = &extract_meds("阿托伐他汀钙片 20mg 每晚一次")[0];
        assert_eq!(o.drug_key.as_deref(), Some("atorvastatin"));
        assert_eq!(o.frequency.as_deref(), Some("qn"));
        assert_eq!(o.frequency_raw.as_deref(), Some("每晚一次"));
        assert_eq!(o.dose_unit.as_deref(), Some("mg"));

        // 每日两次 -> bid.
        let o = &extract_meds("二甲双胍 0.5g 每日两次")[0];
        assert_eq!(o.frequency.as_deref(), Some("bid"));
        assert_eq!(o.frequency_raw.as_deref(), Some("每日两次"));
    }

    #[test]
    fn list_numbered_line_strips_marker_and_route() {
        let o = &extract_meds("1. 盐酸二甲双胍片 0.5g 每日两次口服")[0];
        assert_eq!(o.raw_name, "盐酸二甲双胍片");
        assert_eq!(o.drug_key.as_deref(), Some("metformin"));
        assert_eq!(o.dose_num, Some(0.5));
        assert_eq!(o.dose_unit.as_deref(), Some("g"));
        assert_eq!(o.frequency.as_deref(), Some("bid"));
        // stripped salt/form -> inference, not full confidence.
        assert!((o.confidence - 0.8).abs() < 1e-6);
    }

    #[test]
    fn unmatched_but_has_dose_is_kept() {
        let obs = extract_meds("某未知新药XR 5mg bid");
        assert_eq!(obs.len(), 1);
        let o = &obs[0];
        assert_eq!(o.drug_key, None);
        assert_eq!(o.canonical_name, None);
        assert_eq!(o.rxnorm, None);
        assert_eq!(o.confidence, 0.0);
        assert_eq!(o.dose_num, Some(5.0));
        assert_eq!(o.frequency.as_deref(), Some("bid"));
    }

    #[test]
    fn non_med_prose_line_is_skipped() {
        // No dose, no frequency, no drug match -> not a medication line.
        assert!(extract_meds("患者一般情况可,继续观察。").is_empty());
    }

    #[test]
    fn insulin_units_and_meal_timing() {
        let o = &extract_meds("胰岛素 12U 三餐前")[0];
        assert_eq!(o.dose_num, Some(12.0));
        assert_eq!(o.dose_unit.as_deref(), Some("U"));
        assert_eq!(o.frequency.as_deref(), Some("tid ac"));
        assert_eq!(o.frequency_raw.as_deref(), Some("三餐前"));
    }
}
