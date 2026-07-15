//! Cross-document clinical aggregation (stage B, slice ③).
//!
//! Folds the per-document extractions ([`extract_labs`], [`extract_meds`],
//! [`extract_conditions`]) across MANY source documents into a small "derived
//! layer": analyte trends, medication spans, and a deduped condition list. This
//! is the structure a doctor-summary is later assembled from (slice ④, not this
//! module). Pure in-memory folding: no network, no LLM, no re-parsing of text
//! beyond calling the sibling extractors.
//!
//! ## What this layer does NOT do (kept lean, honest)
//! - **Dates are supplied by the caller**, one per document (e.g. from
//!   [`crate::guess_date`]). We never re-derive a document's clinical date here,
//!   and we never attribute a date to an individual row inside a document — every
//!   row inherits its document's date.
//! - **Medication start/stop/restart is NOT inferred.** `status` is always
//!   `"active"`; there is not enough signal in free-text mentions to detect
//!   discontinuation without inventing it. The proper start/stop/restart fold
//!   (docs/030 §4) is deferred to when the event log carries explicit stop
//!   actions — only then can "stopped" be asserted rather than guessed.
//! - **No cross-synonym merging of conditions.** The dictionary has no condition
//!   category, so conditions are deduped by exact (trimmed) raw text only; two
//!   spellings of the same disease stay separate rather than be laundered.
//! - Unmatched analytes/drugs are kept separate from matched ones (grouped by
//!   raw name) and never merged into a coded series — honest about what resolved.

use crate::{extract_conditions, extract_labs, extract_meds, MedObservation};
use chrono::NaiveDate;
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};

/// One source document to aggregate over. `date` is the document's clinical date
/// (caller supplies it, e.g. from [`crate::guess_date`]); `None` if unknown.
pub struct SourceDoc<'a> {
    /// Stable index back into the caller's record list (kept for evidence).
    pub index: usize,
    pub date: Option<NaiveDate>,
    pub text: &'a str,
}

/// One measured value of an analyte, tagged with the document it came from.
#[derive(Debug, Clone)]
pub struct LabPoint {
    pub date: Option<NaiveDate>,
    /// `value_canonical` if the observation had one, else `value_num`.
    pub value: f64,
    /// `unit_canonical` if present, else `unit_raw`.
    pub unit: Option<String>,
    pub flag: Option<String>,
    /// The [`SourceDoc::index`] this point came from.
    pub source: usize,
}

/// A single analyte's trend across all documents.
#[derive(Debug, Clone)]
pub struct AnalyteSeries {
    /// `Some` for a resolved analyte; `None` when grouped by raw name (unmatched
    /// analytes are kept separate, never merged with matched ones).
    pub analyte_key: Option<String>,
    /// Display/grouping label: canonical name if resolved, else the raw name.
    pub group_name: String,
    pub loinc: Option<String>,
    /// Chronological; `None`-dated points sort last, preserving input order.
    pub points: Vec<LabPoint>,
    /// True if any point is flagged "H" or "L".
    pub any_abnormal: bool,
}

/// A medication's span across all documents that mention it.
#[derive(Debug, Clone)]
pub struct MedSpan {
    /// `Some` for a resolved drug; `None` when grouped by raw name.
    pub drug_key: Option<String>,
    /// Canonical name if resolved, else the raw name.
    pub name: String,
    pub atc: Option<String>,
    /// e.g. "0.5g bid", taken from the most recent mention (fallback: any).
    pub latest_dose: Option<String>,
    /// Earliest dated mention (`None` if no mention carried a date).
    pub start: Option<NaiveDate>,
    /// Latest dated mention.
    pub end: Option<NaiveDate>,
    /// Always "active" — see the module header: discontinuation is not inferred.
    pub status: String,
    /// All [`SourceDoc::index`] that mention it, deduped, ascending.
    pub sources: Vec<usize>,
}

/// A deduped condition mention across documents.
#[derive(Debug, Clone)]
pub struct AggregatedCondition {
    pub raw_text: String,
    /// Earliest dated mention (`None` if no mention carried a date).
    pub onset: Option<NaiveDate>,
    /// All [`SourceDoc::index`] that mention it, deduped, ascending.
    pub sources: Vec<usize>,
}

/// The derived clinical layer: analyte trends, med spans, and conditions.
#[derive(Debug, Clone)]
pub struct AggregatedClinical {
    pub labs: Vec<AnalyteSeries>,
    pub meds: Vec<MedSpan>,
    pub conditions: Vec<AggregatedCondition>,
}

/// Grouping key. `Matched`/`Raw` live in separate namespaces so a resolved item
/// never merges with an unmatched one that happens to share a display string.
#[derive(PartialEq, Eq, Hash, Clone)]
enum GroupKey {
    Matched(String),
    Raw(String),
}

/// Order two optional dates with `None` sorting *after* any `Some` (unknown
/// dates last). Used for both point ordering and output ordering.
fn cmp_date_none_last(a: &Option<NaiveDate>, b: &Option<NaiveDate>) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => x.cmp(y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn min_date(cur: Option<NaiveDate>, new: Option<NaiveDate>) -> Option<NaiveDate> {
    match (cur, new) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, b) => b,
    }
}

fn max_date(cur: Option<NaiveDate>, new: Option<NaiveDate>) -> Option<NaiveDate> {
    match (cur, new) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, b) => b,
    }
}

/// Render a mention's dose + frequency, e.g. "0.5g bid". `None` if the mention
/// carries neither a dose nor a frequency.
fn dose_string(m: &MedObservation) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    match (m.dose_num, &m.dose_unit) {
        (Some(n), Some(u)) => parts.push(format!("{n}{u}")),
        (Some(n), None) => parts.push(format!("{n}")),
        _ => {}
    }
    if let Some(f) = &m.frequency {
        parts.push(f.clone());
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

struct LabBuilder {
    analyte_key: Option<String>,
    group_name: String,
    loinc: Option<String>,
    /// Whether `group_name`/`loinc` were taken from a matched observation yet.
    meta_from_match: bool,
    points: Vec<LabPoint>,
    any_abnormal: bool,
}

struct MedBuilder {
    drug_key: Option<String>,
    name: String,
    atc: Option<String>,
    meta_from_match: bool,
    start: Option<NaiveDate>,
    end: Option<NaiveDate>,
    sources: BTreeSet<usize>,
    /// Dose/date of the mention currently winning "most recent".
    best_dose: Option<String>,
    best_date: Option<NaiveDate>,
    has_best: bool,
}

struct CondBuilder {
    raw_text: String,
    onset: Option<NaiveDate>,
    sources: BTreeSet<usize>,
}

/// Aggregate per-document extractions across `docs` into the derived layer.
pub fn aggregate(docs: &[SourceDoc<'_>]) -> AggregatedClinical {
    let mut labs: HashMap<GroupKey, LabBuilder> = HashMap::new();
    let mut meds: HashMap<GroupKey, MedBuilder> = HashMap::new();
    let mut conds: HashMap<String, CondBuilder> = HashMap::new();

    for doc in docs {
        // --- labs ---
        for obs in extract_labs(doc.text) {
            let matched = obs.analyte_key.is_some();
            let key = match &obs.analyte_key {
                Some(k) => GroupKey::Matched(k.clone()),
                None => GroupKey::Raw(obs.raw_name.clone()),
            };
            let point = LabPoint {
                date: doc.date,
                value: obs.value_canonical.unwrap_or(obs.value_num),
                unit: obs.unit_canonical.clone().or_else(|| obs.unit_raw.clone()),
                flag: obs.flag.clone(),
                source: doc.index,
            };
            let abnormal = matches!(obs.flag.as_deref(), Some("H") | Some("L"));
            let b = labs.entry(key).or_insert_with(|| LabBuilder {
                analyte_key: obs.analyte_key.clone(),
                group_name: obs.raw_name.clone(),
                loinc: None,
                meta_from_match: false,
                points: Vec::new(),
                any_abnormal: false,
            });
            // First matched observation supplies the display name + LOINC.
            if !b.meta_from_match && matched {
                if let Some(name) = &obs.canonical_name {
                    b.group_name = name.clone();
                }
                b.loinc = obs.loinc.clone();
                b.meta_from_match = true;
            }
            b.any_abnormal |= abnormal;
            b.points.push(point);
        }

        // --- meds ---
        for obs in extract_meds(doc.text) {
            let matched = obs.drug_key.is_some();
            let key = match &obs.drug_key {
                Some(k) => GroupKey::Matched(k.clone()),
                None => GroupKey::Raw(obs.raw_name.clone()),
            };
            let this_dose = dose_string(&obs);
            let b = meds.entry(key).or_insert_with(|| MedBuilder {
                drug_key: obs.drug_key.clone(),
                name: obs.raw_name.clone(),
                atc: None,
                meta_from_match: false,
                start: None,
                end: None,
                sources: BTreeSet::new(),
                best_dose: None,
                best_date: None,
                has_best: false,
            });
            if !b.meta_from_match && matched {
                if let Some(name) = &obs.canonical_name {
                    b.name = name.clone();
                }
                b.atc = obs.atc.clone();
                b.meta_from_match = true;
            }
            b.start = min_date(b.start, doc.date);
            b.end = max_date(b.end, doc.date);
            b.sources.insert(doc.index);
            // Keep the dose of the most-recently-dated mention; ties/undated keep
            // the first seen (stable). Fallback: any mention (the first one).
            let replace = if !b.has_best {
                true
            } else {
                match (doc.date, b.best_date) {
                    (Some(m), Some(cur)) => m > cur,
                    (Some(_), None) => true,
                    (None, _) => false,
                }
            };
            if replace {
                b.best_date = doc.date;
                b.best_dose = this_dose;
                b.has_best = true;
            }
        }

        // --- conditions ---
        for c in extract_conditions(doc.text) {
            let raw = c.raw_text.trim().to_string();
            if raw.is_empty() {
                continue;
            }
            let b = conds.entry(raw.clone()).or_insert_with(|| CondBuilder {
                raw_text: raw,
                onset: None,
                sources: BTreeSet::new(),
            });
            b.onset = min_date(b.onset, doc.date);
            b.sources.insert(doc.index);
        }
    }

    // --- finalize labs: chronological points, deterministic series order ---
    let mut lab_out: Vec<AnalyteSeries> = labs
        .into_values()
        .map(|mut b| {
            b.points
                .sort_by(|x, y| cmp_date_none_last(&x.date, &y.date));
            AnalyteSeries {
                analyte_key: b.analyte_key,
                group_name: b.group_name,
                loinc: b.loinc,
                points: b.points,
                any_abnormal: b.any_abnormal,
            }
        })
        .collect();
    // (group_name, analyte_key) fully determinizes order despite HashMap.
    lab_out.sort_by(|a, b| {
        a.group_name
            .cmp(&b.group_name)
            .then_with(|| a.analyte_key.cmp(&b.analyte_key))
    });

    let mut med_out: Vec<MedSpan> = meds
        .into_values()
        .map(|b| MedSpan {
            drug_key: b.drug_key,
            name: b.name,
            atc: b.atc,
            latest_dose: b.best_dose,
            start: b.start,
            end: b.end,
            status: "active".to_string(),
            sources: b.sources.into_iter().collect(),
        })
        .collect();
    med_out.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.drug_key.cmp(&b.drug_key))
    });

    let mut cond_out: Vec<AggregatedCondition> = conds
        .into_values()
        .map(|b| AggregatedCondition {
            raw_text: b.raw_text,
            onset: b.onset,
            sources: b.sources.into_iter().collect(),
        })
        .collect();
    cond_out.sort_by(|a, b| {
        cmp_date_none_last(&a.onset, &b.onset).then_with(|| a.raw_text.cmp(&b.raw_text))
    });

    AggregatedClinical {
        labs: lab_out,
        meds: med_out,
        conditions: cond_out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(y, m, day)
    }

    #[test]
    fn same_analyte_across_docs_forms_one_sorted_series() {
        // 肌酐 (creatinine) in three docs, dates out of order; one abnormal (H).
        let docs = vec![
            SourceDoc {
                index: 0,
                date: d(2023, 6, 1),
                text: "肌酐 96 μmol/L 59-104",
            },
            SourceDoc {
                index: 1,
                date: d(2022, 1, 1),
                text: "肌酐 88 μmol/L 59-104",
            },
            SourceDoc {
                index: 2,
                date: d(2023, 1, 1),
                text: "肌酐 120 μmol/L 59-104", // > 104 -> H
            },
        ];
        let agg = aggregate(&docs);
        assert_eq!(agg.labs.len(), 1);
        let s = &agg.labs[0];
        assert_eq!(s.analyte_key.as_deref(), Some("creatinine"));
        assert!(s.loinc.is_some());
        assert_eq!(s.points.len(), 3);
        // Sorted ascending by date.
        assert_eq!(s.points[0].date, d(2022, 1, 1));
        assert_eq!(s.points[1].date, d(2023, 1, 1));
        assert_eq!(s.points[2].date, d(2023, 6, 1));
        assert_eq!(s.points[0].source, 1);
        assert!(s.any_abnormal, "the 120 point is flagged H");
    }

    #[test]
    fn matched_and_unmatched_analytes_do_not_merge() {
        let docs = vec![
            SourceDoc {
                index: 0,
                date: d(2024, 1, 1),
                text: "肌酐 88 μmol/L 59-104",
            },
            SourceDoc {
                index: 1,
                date: d(2024, 2, 1),
                text: "神秘指标XYZ 12.3 mg/L 0-5",
            },
        ];
        let agg = aggregate(&docs);
        assert_eq!(agg.labs.len(), 2);
        let matched = agg
            .labs
            .iter()
            .find(|s| s.analyte_key.is_some())
            .expect("matched series");
        assert_eq!(matched.analyte_key.as_deref(), Some("creatinine"));
        let unmatched = agg
            .labs
            .iter()
            .find(|s| s.analyte_key.is_none())
            .expect("unmatched series");
        assert_eq!(unmatched.group_name, "神秘指标XYZ");
        assert!(unmatched.loinc.is_none());
        assert_eq!(unmatched.points.len(), 1);
    }

    #[test]
    fn same_drug_across_docs_forms_one_span() {
        let docs = vec![
            SourceDoc {
                index: 3,
                date: d(2023, 1, 1),
                text: "二甲双胍 0.5g bid",
            },
            SourceDoc {
                index: 7,
                date: d(2024, 3, 1),
                text: "二甲双胍 0.85g tid",
            },
        ];
        let agg = aggregate(&docs);
        assert_eq!(agg.meds.len(), 1);
        let m = &agg.meds[0];
        assert_eq!(m.drug_key.as_deref(), Some("metformin"));
        assert_eq!(m.start, d(2023, 1, 1));
        assert_eq!(m.end, d(2024, 3, 1));
        assert_eq!(m.sources, vec![3, 7]);
        assert_eq!(m.status, "active");
        // Dose from the later mention.
        assert_eq!(m.latest_dose.as_deref(), Some("0.85g tid"));
    }

    #[test]
    fn conditions_dedup_with_earliest_onset_and_merged_sources() {
        let docs = vec![
            SourceDoc {
                index: 0,
                date: d(2024, 5, 1),
                text: "出院诊断:2型糖尿病",
            },
            SourceDoc {
                index: 1,
                date: d(2023, 2, 1),
                text: "入院诊断:2型糖尿病",
            },
        ];
        let agg = aggregate(&docs);
        assert_eq!(agg.conditions.len(), 1);
        let c = &agg.conditions[0];
        assert_eq!(c.raw_text, "2型糖尿病");
        assert_eq!(c.onset, d(2023, 2, 1)); // earliest
        assert_eq!(c.sources, vec![0, 1]);
    }

    #[test]
    fn none_dated_lab_point_sorts_last_but_is_kept() {
        let docs = vec![
            SourceDoc {
                index: 0,
                date: None,
                text: "肌酐 88 μmol/L 59-104",
            },
            SourceDoc {
                index: 1,
                date: d(2024, 1, 1),
                text: "肌酐 90 μmol/L 59-104",
            },
        ];
        let agg = aggregate(&docs);
        assert_eq!(agg.labs.len(), 1);
        let s = &agg.labs[0];
        assert_eq!(s.points.len(), 2);
        assert_eq!(s.points[0].date, d(2024, 1, 1));
        assert_eq!(s.points[1].date, None); // undated point last, retained
        assert_eq!(s.points[1].source, 0);
    }

    #[test]
    fn output_vectors_are_in_deterministic_order() {
        // Labs ordered by group_name, meds by name, conditions by onset then text.
        let docs = vec![SourceDoc {
            index: 0,
            date: d(2024, 1, 1),
            text: "\
肌酐 88 μmol/L 59-104
血红蛋白 140 g/L 130-175
二甲双胍 0.5g bid
阿托伐他汀钙片 20mg qn
出院诊断:高血压病；2型糖尿病
",
        }];
        let agg = aggregate(&docs);

        let lab_names: Vec<&str> = agg.labs.iter().map(|s| s.group_name.as_str()).collect();
        let mut sorted_labs = lab_names.clone();
        sorted_labs.sort();
        assert_eq!(lab_names, sorted_labs, "labs must be sorted by group_name");

        let med_names: Vec<&str> = agg.meds.iter().map(|m| m.name.as_str()).collect();
        let mut sorted_meds = med_names.clone();
        sorted_meds.sort();
        assert_eq!(med_names, sorted_meds, "meds must be sorted by name");

        // Both conditions share the doc's onset, so they order by raw_text.
        let cond_texts: Vec<&str> = agg.conditions.iter().map(|c| c.raw_text.as_str()).collect();
        let mut sorted_conds = cond_texts.clone();
        sorted_conds.sort();
        assert_eq!(cond_texts, sorted_conds, "conditions must be sorted");
    }
}
