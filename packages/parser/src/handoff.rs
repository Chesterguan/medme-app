//! Doctor-summary assembly (stage B, slice ④).
//!
//! Turns a share's source documents into the `summary` object the hosted viewer
//! renders (disease swimlanes + trends, docs/030 §3). Deterministic: no network,
//! no LLM. Builds on [`crate::aggregate`] (the derived clinical layer) and a
//! curated **problem → analyte/drug map** ([`problem_map.json`], 10 chronic
//! diseases): labs are grouped under a problem by LOINC, meds by ATC prefix.
//!
//! ## What this does NOT do (kept honest)
//! - **No disease inference.** A problem exists only because a diagnosis line
//!   named it; we merely attach the analytes/meds the guideline map associates
//!   with that disease. Unmapped conditions still become problems (empty labs).
//! - **No fuzzy disease matching.** [`match_disease`] is a plain bidirectional
//!   substring test against the 10 mapped names — no synonym table, no ICD lookup.
//! - **Imaging / pathology impressions are out of scope here** (a later slice);
//!   the viewer's optional `imaging`/`pathology`/`care_facility` EMR fields are
//!   left unset. Only problems / labs / meds / allergies / notable_changes.

use crate::aggregate::{aggregate, AnalyteSeries, MedSpan, SourceDoc};
use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::sync::OnceLock;

/// One lab row in the problem map (only the LOINC is load-bearing here).
#[derive(Deserialize)]
struct MapLab {
    loinc: String,
}

/// One drug class in the problem map (only the ATC prefix is load-bearing).
#[derive(Deserialize)]
struct MapDrug {
    atc: String,
}

/// One mapped chronic disease: its name plus the labs/drugs it groups.
#[derive(Deserialize)]
struct MapEntry {
    disease: String,
    labs: Vec<MapLab>,
    drugs: Vec<MapDrug>,
}

/// Parse the curated problem map once. Serde ignores fields we don't model
/// (icd10, lab name, drug class, source citations).
fn problem_map() -> &'static [MapEntry] {
    static M: OnceLock<Vec<MapEntry>> = OnceLock::new();
    M.get_or_init(|| {
        serde_json::from_str(include_str!("../data/problem_map.json"))
            .expect("problem_map.json is valid")
    })
}

/// Return the mapped disease name if `condition_raw` matches one of the 10
/// mapped chronic diseases, else `None` (the condition still becomes a problem,
/// just without grouped labs/meds).
///
/// Matching rule (deliberately simple + honest): after trimming, a disease
/// matches when its name is a substring of the condition text OR the condition
/// text is a substring of the name. That covers the common shortenings —
/// `"糖尿病"` / `"2型糖尿病"` → `"2型糖尿病"`, `"高血压病3级"` / `"高血压"` →
/// `"高血压"` — without a synonym table. First match in map order wins.
pub fn match_disease(condition_raw: &str) -> Option<&'static str> {
    let c = condition_raw.trim();
    if c.is_empty() {
        return None;
    }
    for e in problem_map() {
        let d = e.disease.as_str();
        if c.contains(d) || d.contains(c) {
            return Some(d);
        }
    }
    None
}

fn entry_for(disease: &str) -> Option<&'static MapEntry> {
    problem_map().iter().find(|e| e.disease == disease)
}

/// Format a value without a trailing `.0` for whole numbers (`88.0` → `"88"`,
/// `7.9` → `"7.9"`), for the human-readable `notable_changes` strings.
fn fmt_num(v: f64) -> String {
    format!("{v}")
}

/// `[["YYYY-MM", value], …]` for the dated points, chronological. Undated points
/// are skipped (the viewer's x-axis is monthly and can't place them).
fn points_json(s: &AnalyteSeries) -> Vec<Value> {
    s.points
        .iter()
        .filter_map(|p| {
            p.date
                .map(|d| json!([d.format("%Y-%m").to_string(), p.value]))
        })
        .collect()
}

/// Distinct source record indices for a series, ascending (for evidence-jump).
fn series_evidence(s: &AnalyteSeries) -> Vec<usize> {
    let set: BTreeSet<usize> = s.points.iter().map(|p| p.source).collect();
    set.into_iter().collect()
}

/// One `labs[]` entry in the viewer schema.
fn series_to_json(s: &AnalyteSeries) -> Value {
    let mut m = Map::new();
    m.insert("name".into(), json!(s.group_name));
    if let Some(u) = s.points.last().and_then(|p| p.unit.clone()) {
        m.insert("unit".into(), json!(u));
    }
    if let Some(h) = s.ref_high {
        m.insert("refHigh".into(), json!(h));
    }
    if let Some(l) = s.ref_low {
        m.insert("refLow".into(), json!(l));
    }
    m.insert("pts".into(), json!(points_json(s)));
    m.insert("evidence".into(), json!(series_evidence(s)));
    Value::Object(m)
}

/// `"自 YYYY-MM"`, optionally `" → YYYY-MM"` when the latest mention is a
/// different month than the earliest. `None` if no mention carried a date.
fn med_span_str(start: Option<NaiveDate>, end: Option<NaiveDate>) -> Option<String> {
    match (start, end) {
        (Some(s), Some(e)) if e != s => {
            Some(format!("自 {} → {}", s.format("%Y-%m"), e.format("%Y-%m")))
        }
        (Some(s), _) => Some(format!("自 {}", s.format("%Y-%m"))),
        (None, Some(e)) => Some(format!("自 {}", e.format("%Y-%m"))),
        (None, None) => None,
    }
}

/// One `meds[]` entry in the viewer schema.
fn med_to_json(m: &MedSpan) -> Value {
    let mut map = Map::new();
    map.insert("name".into(), json!(m.name));
    if let Some(d) = &m.latest_dose {
        map.insert("dose".into(), json!(d));
    }
    map.insert("on".into(), json!(m.status == "active"));
    if let Some(sp) = med_span_str(m.start, m.end) {
        map.insert("span".into(), json!(sp));
    }
    map.insert("evidence".into(), json!(m.sources));
    Value::Object(map)
}

/// Scan `text` for an allergy label (`过敏史` / `过敏`), then split the remainder
/// on `；;，,、` into items of the form `substance(reaction)`; the reaction, if
/// any, is the trailing parenthesized fragment. Negations (`无…`/`否认…`) and
/// empty remainders are skipped. Returns `(substance, reaction)` pairs.
fn extract_allergies_pairs(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        // Prefer the longer label so `过敏史:` isn't split at `过敏`.
        let rest = ["过敏史", "过敏"].iter().find_map(|lbl| {
            line.find(lbl).map(|p| {
                line[p + lbl.len()..]
                    .trim_start_matches(|c: char| c.is_whitespace() || matches!(c, ':' | '：'))
            })
        });
        let Some(rest) = rest else { continue };
        for item in rest.split(['；', ';', '，', ',', '、']) {
            if let Some(pair) = parse_allergy_item(item) {
                out.push(pair);
            }
        }
    }
    out
}

/// Parse one allergy item like `青霉素(皮疹)` → `("青霉素", "皮疹")`, or bare
/// `磺胺` → `("磺胺", "")`. Returns `None` for empty / negation items.
fn parse_allergy_item(item: &str) -> Option<(String, String)> {
    let item = item
        .trim()
        .trim_matches(|c: char| c.is_whitespace() || matches!(c, '。' | '.' | ';' | '；'));
    if item.is_empty() || item.starts_with('无') || item.starts_with("否认") {
        return None;
    }
    if let Some(op) = item.find(['(', '（']) {
        let substance = item[..op].trim().to_string();
        if substance.is_empty() {
            return None;
        }
        let reaction = item[op..]
            .trim_matches(|c: char| matches!(c, '(' | '（' | ')' | '）'))
            .trim()
            .to_string();
        return Some((substance, reaction));
    }
    Some((item.to_string(), String::new()))
}

/// Assemble the deterministic doctor-summary `Value` the viewer consumes.
/// See the module header for scope. `docs[i].index` must equal the record's
/// index in the viewer's `records[]` so evidence chips jump to the right doc.
pub fn assemble_summary(docs: &[SourceDoc<'_>]) -> Value {
    let agg = aggregate(docs);

    // Track which analyte series / med spans got placed under ANY problem, so
    // the leftovers fall into the synthetic「其他」bucket instead of vanishing.
    let mut placed_labs = vec![false; agg.labs.len()];
    let mut placed_meds = vec![false; agg.meds.len()];
    // Grouped-and-abnormal series (with ≥2 points) feed notable_changes.
    let mut changes_pool: Vec<&AnalyteSeries> = Vec::new();

    let mut problems: Vec<Value> = Vec::new();
    // agg.conditions is already sorted by (onset, raw_text) — deterministic.
    for c in &agg.conditions {
        let mut labs_json = Vec::new();
        let mut meds_json = Vec::new();
        let mut warn = false;

        if let Some(disease) = match_disease(&c.raw_text) {
            let entry = entry_for(disease).expect("matched disease is in the map");
            let loincs: BTreeSet<&str> = entry.labs.iter().map(|l| l.loinc.as_str()).collect();
            let prefixes: Vec<&str> = entry
                .drugs
                .iter()
                .map(|d| d.atc.trim_end_matches('*'))
                .collect();

            for (i, s) in agg.labs.iter().enumerate() {
                if s.loinc.as_deref().is_some_and(|l| loincs.contains(l)) {
                    placed_labs[i] = true;
                    warn |= s.any_abnormal;
                    if s.any_abnormal && s.points.len() >= 2 {
                        changes_pool.push(s);
                    }
                    labs_json.push(series_to_json(s));
                }
            }
            for (i, m) in agg.meds.iter().enumerate() {
                if m.atc
                    .as_deref()
                    .is_some_and(|a| prefixes.iter().any(|p| !p.is_empty() && a.starts_with(p)))
                {
                    placed_meds[i] = true;
                    meds_json.push(med_to_json(m));
                }
            }
        }

        let mut prob = Map::new();
        prob.insert("term".into(), json!(c.raw_text));
        if let Some(onset) = c.onset {
            prob.insert("onset".into(), json!(onset.format("%Y-%m").to_string()));
        }
        prob.insert("status".into(), json!(if warn { "需关注" } else { "在管" }));
        prob.insert("warn".into(), json!(warn));
        prob.insert("acute".into(), json!(false));
        prob.insert("evidence".into(), json!(c.sources));
        prob.insert("labs".into(), json!(labs_json));
        prob.insert("meds".into(), json!(meds_json));
        problems.push(Value::Object(prob));
    }

    // ── 其他 bucket: analytes/meds that resolved but map to no problem ──
    let mut other_labs = Vec::new();
    let mut other_warn = false;
    for (i, s) in agg.labs.iter().enumerate() {
        if !placed_labs[i] {
            other_warn |= s.any_abnormal;
            other_labs.push(series_to_json(s));
        }
    }
    let other_meds: Vec<Value> = agg
        .meds
        .iter()
        .enumerate()
        .filter(|(i, _)| !placed_meds[*i])
        .map(|(_, m)| med_to_json(m))
        .collect();
    if !other_labs.is_empty() || !other_meds.is_empty() {
        problems.push(json!({
            "term": "其他",
            "status": "其他",
            "acute": false,
            "warn": other_warn,
            "labs": other_labs,
            "meds": other_meds,
        }));
    }

    // ── notable_changes: short "指标 first→last unit" for abnormal trends ──
    // Deterministic: changes_pool follows agg.labs order (sorted by group_name);
    // cap at 4 to keep it a glance, not a dump.
    let notable_changes: Vec<String> = changes_pool
        .iter()
        .take(4)
        .filter_map(|s| {
            let first = s.points.first()?;
            let last = s.points.last()?;
            let unit = last.unit.clone().unwrap_or_default();
            Some(format!(
                "{} {}→{}{}",
                s.group_name,
                fmt_num(first.value),
                fmt_num(last.value),
                unit
            ))
        })
        .collect();

    // ── allergies: scan every doc, dedup on (substance, reaction) ──
    let mut allergies = Vec::new();
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    for doc in docs {
        for (substance, reaction) in extract_allergies_pairs(doc.text) {
            if seen.insert((substance.clone(), reaction.clone())) {
                allergies.push(json!({ "substance": substance, "reaction": reaction }));
            }
        }
    }

    json!({
        "problems": problems,
        "allergies": allergies,
        "notable_changes": notable_changes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(y, m, day)
    }

    #[test]
    fn match_disease_handles_shortenings_and_nonmatch() {
        assert_eq!(match_disease("糖尿病"), Some("2型糖尿病"));
        assert_eq!(match_disease("2型糖尿病"), Some("2型糖尿病"));
        assert_eq!(match_disease("  2型糖尿病  "), Some("2型糖尿病"));
        assert_eq!(match_disease("高血压"), Some("高血压"));
        assert_eq!(match_disease("高血压病3级"), Some("高血压"));
        assert_eq!(match_disease("社区获得性肺炎"), None);
        assert_eq!(match_disease(""), None);
    }

    #[test]
    fn assemble_summary_groups_labs_meds_and_buckets_the_rest() {
        // doc0/doc1: two HbA1c lab reports (both high) + an unmapped analyte.
        // doc2: a diagnosis note (dates the problem) + a prescription + allergy.
        let docs = vec![
            SourceDoc {
                index: 0,
                date: d(2024, 6, 1),
                text: "生化检验报告单\n糖化血红蛋白 7.9 % 4-6.5\n神秘指标XYZ 12.3 mg/L 0-5",
            },
            SourceDoc {
                index: 1,
                date: d(2026, 6, 1),
                text: "生化检验报告单\n糖化血红蛋白 7.2 % 4-6.5",
            },
            SourceDoc {
                index: 2,
                date: d(2021, 5, 1),
                text: "门诊病历\n诊断:2型糖尿病\n二甲双胍 0.5g bid\n过敏史:青霉素(皮疹)",
            },
        ];
        let sm = assemble_summary(&docs);

        let problems = sm["problems"].as_array().expect("problems array");
        let dm = problems
            .iter()
            .find(|p| p["term"] == "2型糖尿病")
            .expect("2型糖尿病 problem present");
        assert_eq!(dm["onset"], "2021-05");
        assert_eq!(dm["evidence"], json!([2]));

        // Grouped HbA1c lab: refHigh present, two chronological points.
        let labs = dm["labs"].as_array().expect("labs");
        let hba1c = labs
            .iter()
            .find(|l| l["name"] == "糖化血红蛋白")
            .expect("HbA1c grouped under diabetes");
        assert_eq!(hba1c["refHigh"], json!(6.5));
        let pts = hba1c["pts"].as_array().expect("pts");
        assert_eq!(pts.len(), 2);
        assert_eq!(pts[0], json!(["2024-06", 7.9]));
        assert_eq!(pts[1], json!(["2026-06", 7.2]));

        // Grouped metformin med, currently on.
        let meds = dm["meds"].as_array().expect("meds");
        let met = meds
            .iter()
            .find(|m| m["name"] == "二甲双胍")
            .expect("metformin grouped under diabetes");
        assert_eq!(met["on"], json!(true));

        // Unmapped analyte falls into the 其他 bucket.
        let other = problems
            .iter()
            .find(|p| p["term"] == "其他")
            .expect("其他 bucket present");
        assert!(other["labs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["name"] == "神秘指标XYZ"));

        // notable_changes summarizes the abnormal HbA1c trend.
        let changes = sm["notable_changes"].as_array().expect("notable_changes");
        assert!(!changes.is_empty());
        assert!(changes[0].as_str().unwrap().contains("糖化血红蛋白"));

        // Allergy parsed with its reaction.
        let allergies = sm["allergies"].as_array().expect("allergies");
        assert_eq!(allergies.len(), 1);
        assert_eq!(allergies[0]["substance"], "青霉素");
        assert_eq!(allergies[0]["reaction"], "皮疹");
    }

    #[test]
    fn unmapped_condition_still_becomes_a_problem_without_groups() {
        let docs = vec![SourceDoc {
            index: 0,
            date: d(2022, 12, 1),
            text: "出院诊断:社区获得性肺炎",
        }];
        let sm = assemble_summary(&docs);
        let problems = sm["problems"].as_array().unwrap();
        let p = problems
            .iter()
            .find(|p| p["term"] == "社区获得性肺炎")
            .expect("unmapped condition is still a problem");
        assert_eq!(p["labs"], json!([]));
        assert_eq!(p["meds"], json!([]));
        assert_eq!(p["warn"], json!(false));
        assert_eq!(p["status"], "在管");
    }

    #[test]
    fn allergy_negation_and_bare_substance() {
        // Negations are skipped; a bare substance has an empty reaction.
        assert!(extract_allergies_pairs("过敏史:无").is_empty());
        assert!(extract_allergies_pairs("否认药物过敏史").is_empty());
        let pairs = extract_allergies_pairs("过敏史:磺胺、头孢(荨麻疹)");
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0], ("磺胺".to_string(), String::new()));
        assert_eq!(pairs[1], ("头孢".to_string(), "荨麻疹".to_string()));
    }
}
