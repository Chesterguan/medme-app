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

// Reference-range regexes match ANYWHERE in the trailing columns (not anchored),
// and tolerate spaces around the comparator/dash. 真 corpus 把参考写成带空格的
// `< 5.20`、`> 90`、`3.9 - 6.1` —— 逐 token 分类会把 `<` 和 `5.20` 拆成两个各自作废
// 的 token,单边参考(LDL/TC/eGFR)与带空格的双边参考就全丢了(quality dim 4/5)。
fn range_two_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(\d+(?:\.\d+)?)\s*[-~]\s*(\d+(?:\.\d+)?)").expect("range re")
    })
}
fn range_high_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"[<≤]=?\s*(\d+(?:\.\d+)?)").expect("high re"))
}
fn range_low_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"[>≥]=?\s*(\d+(?:\.\d+)?)").expect("low re"))
}
/// A dash-separated `YYYY-MM-DD` date. Only the dash form matters: the range regex
/// keys on `[-~]`, so slash/dot dates never look like a range in the first place.
/// A real reference range has a single dash (`3.9-6.1`); a date has two — so this
/// blanks a trailing 采样/报告日期 (`… 2024-01-05`) without ever eating a range,
/// stopping it from being misread as `2024-1` and fabricating a flag. Quality dim 4/5.
fn date_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\d{2,4}-\d{1,2}-\d{1,2}").expect("date re"))
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

/// Locate the reference range anywhere in the trailing columns. Returns
/// `(low, high, byte_span_in_folded)`: `59-104`/`3.9 - 6.1` → both bounds;
/// `< 5.20`/`≤6.5` → high only; `> 90`/`≥130` → low only. Two-sided wins over
/// single-sided. `None` when no range is present. `folded` must already be
/// punctuation-folded so `＜`/`～`/`－` read as `<`/`~`/`-`.
fn find_range(folded: &str) -> Option<(Option<f64>, Option<f64>, (usize, usize))> {
    if let Some(c) = range_two_re().captures(folded) {
        let lo = c.get(1)?.as_str().parse().ok();
        let hi = c.get(2)?.as_str().parse().ok();
        let m = c.get(0)?;
        return Some((lo, hi, (m.start(), m.end())));
    }
    if let Some(c) = range_high_re().captures(folded) {
        let hi = c.get(1)?.as_str().parse().ok();
        let m = c.get(0)?;
        return Some((None, hi, (m.start(), m.end())));
    }
    if let Some(c) = range_low_re().captures(folded) {
        let lo = c.get(1)?.as_str().parse().ok();
        let m = c.get(0)?;
        return Some((lo, None, (m.start(), m.end())));
    }
    None
}

/// Parse the trailing `单位 参考范围 [↑/↓]` columns. The reference range is matched
/// on the whole (punctuation-folded) string first — so a spaced `< 5.20` or
/// `3.9 - 6.1` parses as one range — then blanked out; unit and flag are read from
/// what remains, order-independently.
fn parse_rest(rest: &str) -> (Option<String>, Option<f64>, Option<f64>, Option<String>) {
    let folded = fold_range_punct(rest);
    // Blank any embedded date so the unanchored range scan can't read it as a range.
    let folded = date_re().replace_all(&folded, " ").into_owned();
    let (mut low, mut high) = (None, None);
    let mut scan = folded.clone();
    if let Some((lo, hi, (s, e))) = find_range(&folded) {
        low = lo;
        high = hi;
        // Blank the range so its digits can't be re-read as a unit token.
        scan.replace_range(s..e, " ");
    }

    let (mut unit, mut flag) = (None, None);
    for raw in scan.split_whitespace() {
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
        // A real analyte name has no *sentence* punctuation. This rejects narrative
        // fragments that a mis-routed prose/imaging line would otherwise smuggle in
        // as a "lab" (`右肺上叶尖段磨玻璃结节(GGN),大小约` value 8 …) — quality dim 3.
        if raw_name
            .chars()
            .any(|c| matches!(c, '，' | ',' | '。' | '；' | ';' | '、'))
        {
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
    fn spaced_single_sided_and_two_sided_refs_parse() {
        // 真 corpus 的写法:单边参考带空格 `< 5.20` / `> 90`,双边参考带空格 `3.9 - 6.1`。
        // 过去逐 token 分类把它们拆碎全丢;现在都要解析出 refLow/refHigh。
        let text = "\
TC         总胆固醇 Cholesterol   6.05     mmol/L      < 5.20          ↑
eGFR       估算肾小球滤过率      72       ml/min/1.73m2   > 90         ↓
GLU        空腹血糖 Glucose       7.1      mmol/L      3.9 - 6.1       ↑
";
        let obs = extract_labs(text);
        let tc = find(&obs, "cholesterol");
        assert_eq!(tc.ref_high, Some(5.20));
        assert_eq!(tc.ref_low, None);
        assert_eq!(tc.unit_raw.as_deref(), Some("mmol/L"));
        assert_eq!(tc.flag.as_deref(), Some("H"));
        let egfr = find(&obs, "egfr");
        assert_eq!(egfr.ref_low, Some(90.0));
        assert_eq!(egfr.ref_high, None);
        assert_eq!(egfr.flag.as_deref(), Some("L"));
        let glu = find(&obs, "glucose");
        assert_eq!(glu.ref_low, Some(3.9));
        assert_eq!(glu.ref_high, Some(6.1));
    }

    #[test]
    fn trailing_report_date_is_not_read_as_a_reference_range() {
        // 行尾的采样/报告日期(`2024-01-05`)不得被无锚点的范围扫描当成参考范围
        // `2024-1`,否则会伪造 refLow/refHigh 并派生出假异常 flag。
        let obs = extract_labs("葡萄糖 Glucose 5.0 mmol/L 2024-01-05");
        let glu = find(&obs, "glucose");
        assert_eq!(glu.ref_low, None);
        assert_eq!(glu.ref_high, None);
        assert_eq!(glu.flag, None);
        // 真参考范围与行尾日期并存时,仍解析出参考范围,且不被日期污染。
        let obs2 = extract_labs("葡萄糖 Glucose 7.1 mmol/L 3.9 - 6.1 ↑ 2024-01-05");
        let glu2 = find(&obs2, "glucose");
        assert_eq!(glu2.ref_low, Some(3.9));
        assert_eq!(glu2.ref_high, Some(6.1));
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
