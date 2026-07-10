//! Clinical terminology normalization layer.
//!
//! A static, versioned dictionary (`dictionary.json`, compiled in via
//! `include_str!`) plus a single lookup function [`normalize`]. It maps a raw
//! Chinese/English/abbreviation/OCR-split term to an internal canonical key +
//! canonical Chinese name + international codes (LOINC / RxNorm / ATC + OMOP
//! standard concept_id) + canonical unit + explicit unit conversions.
//!
//! This layer has no runtime "value": it does not perform conversions itself.
//! Instead each accepted source unit carries an affine conversion written into
//! the data, so any consumer can compute with zero ambiguity:
//!
//! ```text
//! canonical_value = slope * source_value + intercept
//! ```
//!
//! Design: `docs/superpowers/specs/2026-07-10-terminology-normalization-layer-design.md`.

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

/// The compiled-in dictionary. A parse failure here is a build-time bug in the
/// shipped resource, not a runtime condition — see [`index`].
const DICTIONARY_JSON: &str = include_str!("../dictionary.json");

/// What kind of clinical concept an entry describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Lab,
    Vital,
    Drug,
}

/// International codes for a concept. Each is a separate slot — multiple coding
/// systems are never collapsed into one field (design §6 red line 4). Absent
/// codes are `None`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Codes {
    #[serde(default)]
    pub loinc: Option<String>,
    #[serde(default)]
    pub rxnorm: Option<String>,
    #[serde(default)]
    pub atc: Option<String>,
    #[serde(default)]
    pub omop_concept_id: Option<i64>,
}

/// One accepted source unit and its affine conversion to the entry's
/// `canonical_unit`: `canonical_value = slope * source_value + intercept`.
/// The canonical unit itself is the row `slope = 1, intercept = 0`.
#[derive(Debug, Clone, Deserialize)]
pub struct UnitConversion {
    /// UCUM unit notation, e.g. `umol/L`, `mg/dL`, `10*9/L`, `mmol/mol`.
    pub unit: String,
    pub slope: f64,
    pub intercept: f64,
}

/// A single dictionary entry (lab / vital / drug). Lab and vital entries carry
/// `system` / `canonical_unit` / `units`; drug entries carry `ingredient`.
#[derive(Debug, Clone, Deserialize)]
pub struct Entry {
    /// Internal canonical key, e.g. `creatinine`, `metformin`.
    pub key: String,
    /// Canonical Chinese display name.
    pub canonical_name: String,
    pub category: Category,
    /// LOINC specimen, e.g. `serum/plasma` — keeps serum ≠ urine from
    /// collapsing (design §6 red line 2). `None` for drugs.
    #[serde(default)]
    pub system: Option<String>,
    pub codes: Codes,
    /// Canonical unit (UCUM). `None` for drugs.
    #[serde(default)]
    pub canonical_unit: Option<String>,
    /// Explicit conversions; empty for drugs.
    #[serde(default)]
    pub units: Vec<UnitConversion>,
    /// Active ingredient (English); `Some` only for drugs.
    #[serde(default)]
    pub ingredient: Option<String>,
    /// Exact aliases — a normalized hit yields confidence 1.0.
    pub aliases: Vec<String>,
    /// Known OCR misreads — a normalized hit yields confidence 0.5 (suspect,
    /// routed to human review rather than trusted).
    #[serde(default)]
    pub ocr_confusions: Vec<String>,
    /// Human-readable caveat about this entry's coding/conversion choices, e.g.
    /// a deliberate non-standard concept or a source-unit assumption.
    #[serde(default)]
    pub note: Option<String>,
}

/// Top-level dictionary document.
#[derive(Debug, Clone, Deserialize)]
pub struct Dictionary {
    pub version: String,
    pub entries: Vec<Entry>,
}

/// A successful normalization. Mapping is always *additive*: the caller keeps
/// the original raw term + span; this only annotates it (design §6 red line 1).
#[derive(Debug, Clone)]
pub struct Match {
    pub key: String,
    pub canonical_name: String,
    pub category: Category,
    pub codes: Codes,
    /// Active ingredient (drugs only).
    pub ingredient: Option<String>,
    /// The dictionary alias that matched (traceable back to the data).
    pub matched_alias: String,
    /// 1.0 for an exact alias hit, 0.5 for an OCR-confusion hit.
    pub confidence: f32,
}

/// A resolved alias: which entry it belongs to and the original alias text.
struct AliasHit {
    entry_idx: usize,
    alias: String,
}

/// Lazily-built lookup index over the dictionary.
struct Index {
    entries: Vec<Entry>,
    /// normalized alias -> hit (confidence 1.0).
    aliases: HashMap<String, AliasHit>,
    /// normalized ocr_confusion -> hit (confidence 0.5).
    confusions: HashMap<String, AliasHit>,
}

/// Normalization applied to BOTH index keys and query terms so lookups are
/// insensitive to case, full-width forms, and internal whitespace (OCR often
/// splits CJK, e.g. `肌 酐` -> `肌酐`). This is the single shared helper the
/// design mandates.
fn normalize_term(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        let ch = to_halfwidth(ch);
        if ch.is_whitespace() {
            continue;
        }
        for lc in ch.to_lowercase() {
            out.push(lc);
        }
    }
    out
}

/// Map a full-width character to its ASCII half-width equivalent. The
/// ideographic space (U+3000) folds to a normal space (then stripped by
/// `normalize_term`); full-width `!`..`~` (U+FF01..U+FF5E) map to ASCII
/// 0x21..0x7E. All other characters pass through unchanged.
fn to_halfwidth(c: char) -> char {
    match c {
        '\u{3000}' => ' ',
        '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
        _ => c,
    }
}

/// Parse the compiled-in dictionary and build the alias/confusion indexes.
///
/// Invariant: `dictionary.json` is a shipped, version-controlled resource that
/// is validated by this crate's tests (including `parse_dictionary` and
/// `no_duplicate_alias_across_entries`). A parse failure or duplicate alias is
/// therefore a build-time bug, so `expect` documents that invariant rather than
/// propagating an error that no runtime caller could act on.
fn build_index() -> Index {
    let dict: Dictionary = serde_json::from_str(DICTIONARY_JSON)
        .expect("dictionary.json is a valid, shipped resource");

    let mut aliases: HashMap<String, AliasHit> = HashMap::new();
    let mut confusions: HashMap<String, AliasHit> = HashMap::new();

    for (entry_idx, entry) in dict.entries.iter().enumerate() {
        for alias in &entry.aliases {
            let norm = normalize_term(alias);
            aliases.insert(
                norm,
                AliasHit {
                    entry_idx,
                    alias: alias.clone(),
                },
            );
        }
        for confusion in &entry.ocr_confusions {
            let norm = normalize_term(confusion);
            confusions.insert(
                norm,
                AliasHit {
                    entry_idx,
                    alias: confusion.clone(),
                },
            );
        }
    }

    Index {
        entries: dict.entries,
        aliases,
        confusions,
    }
}

fn index() -> &'static Index {
    static INDEX: OnceLock<Index> = OnceLock::new();
    INDEX.get_or_init(build_index)
}

impl Index {
    fn to_match(&self, hit: &AliasHit, confidence: f32) -> Match {
        let e = &self.entries[hit.entry_idx];
        Match {
            key: e.key.clone(),
            canonical_name: e.canonical_name.clone(),
            category: e.category,
            codes: e.codes.clone(),
            ingredient: e.ingredient.clone(),
            matched_alias: hit.alias.clone(),
            confidence,
        }
    }
}

/// Map a single candidate term to its canonical concept. Returns `None` on no
/// hit. This is a lookup, not a full-text scan — locating terms in free text is
/// the extraction layer's job (design §6).
///
/// An exact (normalized) alias hit yields `confidence == 1.0`; an
/// `ocr_confusions` hit yields `confidence == 0.5`.
pub fn normalize(raw_term: &str) -> Option<Match> {
    let norm = normalize_term(raw_term);
    if norm.is_empty() {
        return None;
    }
    let idx = index();
    if let Some(hit) = idx.aliases.get(&norm) {
        return Some(idx.to_match(hit, 1.0));
    }
    if let Some(hit) = idx.confusions.get(&norm) {
        return Some(idx.to_match(hit, 0.5));
    }
    None
}

/// Read-only access to the parsed dictionary (entries), for consumers that need
/// to enumerate concepts (e.g. entity search auto-complete).
pub fn dictionary_entries() -> &'static [Entry] {
    &index().entries
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every analyte/vital/drug key required by docs/015 §3.5. The dictionary
    /// may add siblings, but must never drop one of these.
    const REQUIRED_KEYS: &[&str] = &[
        // labs: renal, glucose, lipids, liver, CBC, thyroid
        "creatinine",
        "egfr",
        "urea",
        "uric_acid",
        "glucose",
        "hba1c",
        "cholesterol",
        "ldl",
        "hdl",
        "triglycerides",
        "alt",
        "ast",
        "tbil",
        "albumin",
        "wbc",
        "hgb",
        "plt",
        "neut_pct",
        "tsh",
        // vitals
        "bp_systolic",
        "bp_diastolic",
        "heart_rate",
        "body_weight",
        "bmi",
        // drugs named in §3.5
        "metformin",
        "glimepiride",
        "acarbose",
        "empagliflozin",
        "semaglutide",
        "insulin_glargine",
        "insulin_aspart",
        "valsartan",
        "amlodipine",
        "metoprolol",
        "hydrochlorothiazide",
        "perindopril",
        "atorvastatin",
        "rosuvastatin",
        "ezetimibe",
        "aspirin",
        "clopidogrel",
        "warfarin",
        "rivaroxaban",
        "allopurinol",
        "levothyroxine",
        "pantoprazole",
    ];

    #[test]
    fn parse_dictionary() {
        let dict: Dictionary = serde_json::from_str(DICTIONARY_JSON).expect("valid dictionary");
        assert!(!dict.version.is_empty());
        assert!(dict.entries.len() >= 50, "unexpectedly few entries");
    }

    #[test]
    fn alias_hits_map_to_same_key() {
        // 谷丙转氨酶 / ALT / GPT / SGPT -> alt
        for t in ["谷丙转氨酶", "ALT", "GPT", "SGPT"] {
            let m = normalize(t).unwrap_or_else(|| panic!("no match for {t}"));
            assert_eq!(m.key, "alt", "term {t}");
            assert_eq!(m.confidence, 1.0);
            assert_eq!(m.category, Category::Lab);
        }
        // 肌酐 / 血肌酐 / Cr / SCr -> creatinine
        for t in ["肌酐", "血肌酐", "Cr", "SCr"] {
            let m = normalize(t).unwrap_or_else(|| panic!("no match for {t}"));
            assert_eq!(m.key, "creatinine", "term {t}");
            assert_eq!(m.confidence, 1.0);
        }
    }

    #[test]
    fn normalization_case_fullwidth_and_split() {
        // full-width ＡＬＴ -> alt
        assert_eq!(normalize("ＡＬＴ").unwrap().key, "alt");
        // lowercase crea -> creatinine
        assert_eq!(normalize("crea").unwrap().key, "creatinine");
        // OCR-split 肌 酐 (internal space stripped) -> creatinine
        assert_eq!(normalize("肌 酐").unwrap().key, "creatinine");
        // full-width ideographic space also stripped
        assert_eq!(normalize("肌\u{3000}酐").unwrap().key, "creatinine");
    }

    #[test]
    fn ocr_confusion_hits_are_low_confidence() {
        let m = normalize("肌研").expect("ocr confusion should match");
        assert_eq!(m.key, "creatinine");
        assert_eq!(m.confidence, 0.5);
        assert_eq!(m.matched_alias, "肌研");
    }

    #[test]
    fn miss_returns_none() {
        assert!(normalize("完全不是术语").is_none());
        assert!(normalize("").is_none());
        assert!(normalize("   ").is_none());
    }

    #[test]
    fn matched_alias_is_the_dictionary_form() {
        // Query normalization must not leak into matched_alias: it reports the
        // dictionary's original alias, for traceability.
        let m = normalize("creatinine").unwrap();
        assert_eq!(m.matched_alias, "Creatinine");
    }

    #[test]
    fn drug_carries_ingredient_and_codes() {
        let m = normalize("格华止").unwrap();
        assert_eq!(m.key, "metformin");
        assert_eq!(m.category, Category::Drug);
        assert_eq!(m.ingredient.as_deref(), Some("Metformin"));
        assert_eq!(m.codes.rxnorm.as_deref(), Some("6809"));
        assert_eq!(m.codes.atc.as_deref(), Some("A10BA02"));
        assert_eq!(m.codes.omop_concept_id, Some(1503297));
        // drugs have no LOINC / canonical unit
        assert!(m.codes.loinc.is_none());
    }

    fn entry_for(key: &str) -> &'static Entry {
        dictionary_entries()
            .iter()
            .find(|e| e.key == key)
            .unwrap_or_else(|| panic!("missing entry {key}"))
    }

    #[test]
    fn creatinine_conversion_is_correct() {
        let e = entry_for("creatinine");
        assert_eq!(e.canonical_unit.as_deref(), Some("umol/L"));
        // canonical row is identity
        let canonical = e.units.iter().find(|u| u.unit == "umol/L").unwrap();
        assert_eq!(canonical.slope, 1.0);
        assert_eq!(canonical.intercept, 0.0);
        // mg/dL -> umol/L is the molar factor 88.42 (linear, intercept 0)
        let mgdl = e.units.iter().find(|u| u.unit == "mg/dL").unwrap();
        assert_eq!(mgdl.slope, 88.42);
        assert_eq!(mgdl.intercept, 0.0);
    }

    #[test]
    fn hba1c_conversion_is_affine_not_a_plain_factor() {
        let e = entry_for("hba1c");
        assert_eq!(e.canonical_unit.as_deref(), Some("%"));
        // IFCC mmol/mol -> NGSP %: NGSP% = 0.09148 * IFCC + 2.152 (AFFINE).
        // A plain factor (intercept 0) would be clinically wrong.
        let ifcc = e.units.iter().find(|u| u.unit == "mmol/mol").unwrap();
        assert_eq!(ifcc.slope, 0.09148);
        assert_eq!(ifcc.intercept, 2.152);
        assert!(
            ifcc.intercept != 0.0,
            "HbA1c IFCC->NGSP must be affine, not a plain factor"
        );
        // Spot-check the value: IFCC 53 mmol/mol ~= NGSP 7.0 %.
        let ngsp = ifcc.slope * 53.0 + ifcc.intercept;
        assert!((ngsp - 7.0).abs() < 0.05, "got {ngsp}");
    }

    #[test]
    fn completeness_all_required_items_present() {
        let keys: std::collections::HashSet<&str> = dictionary_entries()
            .iter()
            .map(|e| e.key.as_str())
            .collect();
        for req in REQUIRED_KEYS {
            assert!(keys.contains(req), "docs/015 §3.5 item missing: {req}");
        }
    }

    #[test]
    fn total_entry_count_is_expected() {
        // Coverage expansion (2026-07-10.2): 54 original + 137 curated = 191.
        // A drift here means an entry was accidentally dropped or duplicated.
        assert_eq!(
            dictionary_entries().len(),
            191,
            "unexpected dictionary entry count"
        );
    }

    #[test]
    fn new_coverage_keys_resolve() {
        // Representative sample across the six new fragments (chemistry / heme /
        // endocrine-cardiac / urine-tumor-vitamin / vitals-drugs / drugs): each
        // must normalize() to the right key and category at full confidence.
        let cases: &[(&str, &str, Category)] = &[
            ("血钾", "potassium", Category::Lab),
            ("HCT", "hct", Category::Lab),
            ("FT3", "ft3", Category::Lab),
            ("CA125", "ca125", Category::Lab),
            ("尿蛋白", "urine_protein", Category::Lab),
            ("SpO2", "spo2", Category::Vital),
            ("替米沙坦", "telmisartan", Category::Drug),
            ("奥美拉唑", "omeprazole", Category::Drug),
        ];
        for (term, key, cat) in cases {
            let m = normalize(term).unwrap_or_else(|| panic!("no match for {term}"));
            assert_eq!(m.key, *key, "term {term}");
            assert_eq!(m.category, *cat, "term {term} category");
            assert_eq!(m.confidence, 1.0, "term {term} confidence");
        }
    }

    #[test]
    fn labs_and_vitals_have_canonical_unit_and_identity_row() {
        for e in dictionary_entries() {
            match e.category {
                // A lab/vital is either QUANTITATIVE (canonical_unit is Some) or
                // QUALITATIVE (canonical_unit is None, e.g. a urinalysis dipstick
                // ordinal like -/+/++). Both are valid; each has its own rule.
                Category::Lab | Category::Vital => match e.canonical_unit.as_deref() {
                    // Quantitative: there must be a units[] row for the canonical
                    // unit itself, and it must be the identity (slope 1, intercept 0).
                    Some(cu) => {
                        let row = e.units.iter().find(|u| u.unit == cu).unwrap_or_else(|| {
                            panic!("{} has no units row for canonical {cu}", e.key)
                        });
                        assert_eq!(row.slope, 1.0, "{} canonical row slope", e.key);
                        assert_eq!(row.intercept, 0.0, "{} canonical row intercept", e.key);
                    }
                    // Qualitative: no canonical unit means there is nothing to
                    // convert, so units[] MUST be empty — a conversion row here
                    // would be a bogus numeric mapping over an ordinal result.
                    None => {
                        assert!(
                            e.units.is_empty(),
                            "{} qualitative lab must have empty units (no bogus conversions)",
                            e.key
                        );
                    }
                },
                Category::Drug => {
                    // drugs carry no unit machinery, but must carry an ingredient
                    assert!(e.ingredient.is_some(), "{} drug missing ingredient", e.key);
                    assert!(
                        e.canonical_unit.is_none(),
                        "{} drug has canonical_unit",
                        e.key
                    );
                    assert!(e.units.is_empty(), "{} drug has units", e.key);
                }
            }
        }
    }

    #[test]
    fn loinc_property_agrees_with_canonical_unit() {
        // Design §5/§7: a lab's LOINC property/scale must not contradict its
        // canonical unit. Enforce the molar<->µmol/mmol and mass<->mg/g rule for
        // every entry whose canonical unit implies a substance-concentration
        // property, by asserting our deliberate molar-LOINC choices.
        let molar: &[(&str, &str, &str)] = &[
            ("creatinine", "14682-9", "umol/L"),
            ("urea", "22664-7", "mmol/L"),
            ("uric_acid", "14933-6", "umol/L"),
            ("glucose", "14749-6", "mmol/L"),
            ("cholesterol", "14647-2", "mmol/L"),
            ("ldl", "22748-8", "mmol/L"),
            ("hdl", "14646-4", "mmol/L"),
            ("triglycerides", "14927-8", "mmol/L"),
            ("tbil", "14631-6", "umol/L"),
            // New coverage — electrolytes, canonical molar mmol/L.
            ("potassium", "2823-3", "mmol/L"),
            ("sodium", "2951-2", "mmol/L"),
            ("chloride", "2075-0", "mmol/L"),
            ("calcium", "2000-8", "mmol/L"),
            ("phosphate", "14879-1", "mmol/L"),
            ("magnesium", "2601-3", "mmol/L"),
            ("bicarbonate", "1963-8", "mmol/L"),
            // Bilirubin fractions, canonical molar umol/L.
            ("direct_bilirubin", "14629-0", "umol/L"),
            ("indirect_bilirubin", "14630-8", "umol/L"),
            // Other clearly-molar analytes.
            ("homocysteine", "13965-9", "umol/L"),
            ("serum_iron", "14798-3", "umol/L"),
            // SI vitamins — mole-based canonicals (nmol/L, pmol/L).
            ("vitamin_d_25oh", "68438-1", "nmol/L"),
            ("vitamin_b12", "14685-2", "pmol/L"),
            ("folate", "14732-2", "nmol/L"),
        ];
        for (key, loinc, unit) in molar {
            let e = entry_for(key);
            assert_eq!(e.codes.loinc.as_deref(), Some(*loinc), "{key} loinc");
            assert_eq!(e.canonical_unit.as_deref(), Some(*unit), "{key} unit");
            // molar canonical must be a mole-based concentration
            assert!(
                unit.starts_with("pmol")
                    || unit.starts_with("nmol")
                    || unit.starts_with("umol")
                    || unit.starts_with("mmol"),
                "{key} canonical unit not molar"
            );
        }
    }

    #[test]
    fn no_duplicate_alias_across_entries() {
        // Fail loudly rather than silently shadow: no normalized alias may be
        // claimed by two different entries (design §6 / §7). Also guards
        // ocr_confusions from colliding with aliases.
        let mut seen: HashMap<String, String> = HashMap::new();
        for e in dictionary_entries() {
            for a in e.aliases.iter().chain(e.ocr_confusions.iter()) {
                let norm = normalize_term(a);
                if let Some(prev) = seen.get(&norm) {
                    assert_eq!(
                        prev, &e.key,
                        "duplicate normalized alias {norm:?} in entries {prev} and {}",
                        e.key
                    );
                }
                seen.insert(norm, e.key.clone());
            }
        }
    }
}
