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
    /// 1.0 = 字典别名精确命中;0.8 = **剥壳推断**(去盐基/剂型/规格/载液后才命中,是推断
    /// 不是原文);0.5 = OCR 混淆表命中(可疑,送人工复核)。上层据此决定信不信。
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
    /// normalized alias -> hit (confidence 1.0)。**化验/体征优先**:见 [`build_index`]。
    aliases: HashMap<String, AliasHit>,
    /// normalized ocr_confusion -> hit (confidence 0.5).
    confusions: HashMap<String, AliasHit>,
    /// 只含 drug 条目的别名表 —— 处方语境用 [`normalize_drug`] 查这张,
    /// 免得「叶酸」「氢化可的松」被同名的化验项抢走。
    drug_aliases: HashMap<String, AliasHit>,
    /// drug 的 OCR 混淆表(同上,按类别分)。
    drug_confusions: HashMap<String, AliasHit>,
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
/// `no_duplicate_alias_within_category`). A parse failure or duplicate alias is
/// therefore a build-time bug, so `expect` documents that invariant rather than
/// propagating an error that no runtime caller could act on.
///
/// 同名跨类别是**真实存在**的:「叶酸」既是化验(血清叶酸)也是药(叶酸片);「氢化可的松」
/// 既是化验(皮质醇)也是药。别名表按类别
/// 分开建:无类别信息的 [`normalize`] 让**化验/体征优先**(报告单里这些词绝大多数是化验项),
/// 处方语境改用 [`normalize_drug`] 查 drug 专表。同一类别内别名仍必须唯一
/// (`no_duplicate_alias_within_category` 守着)。
fn build_index() -> Index {
    let dict: Dictionary = serde_json::from_str(DICTIONARY_JSON)
        .expect("dictionary.json is a valid, shipped resource");

    let mut aliases: HashMap<String, AliasHit> = HashMap::new();
    let mut confusions: HashMap<String, AliasHit> = HashMap::new();
    let mut drug_aliases: HashMap<String, AliasHit> = HashMap::new();
    let mut drug_confusions: HashMap<String, AliasHit> = HashMap::new();

    for (entry_idx, entry) in dict.entries.iter().enumerate() {
        let is_drug = entry.category == Category::Drug;
        for alias in &entry.aliases {
            let norm = normalize_term(alias);
            let hit = AliasHit {
                entry_idx,
                alias: alias.clone(),
            };
            if is_drug {
                // 化验优先:通用表里已有(必是化验/体征)就不覆盖。
                drug_aliases.insert(norm.clone(), hit);
                aliases.entry(norm).or_insert_with(|| AliasHit {
                    entry_idx,
                    alias: alias.clone(),
                });
            } else {
                aliases.insert(norm, hit);
            }
        }
        for confusion in &entry.ocr_confusions {
            let norm = normalize_term(confusion);
            let hit = AliasHit {
                entry_idx,
                alias: confusion.clone(),
            };
            if is_drug {
                // 与 aliases 同样的分表规则:混淆表也不能让药悄悄盖掉化验项。
                drug_confusions.insert(norm.clone(), hit);
                confusions.entry(norm).or_insert_with(|| AliasHit {
                    entry_idx,
                    alias: confusion.clone(),
                });
            } else {
                confusions.insert(norm, hit);
            }
        }
    }

    Index {
        entries: dict.entries,
        aliases,
        confusions,
        drug_aliases,
        drug_confusions,
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

/// 剥壳推断出来的命中置信度:低于精确命中(1.0),高于 OCR 混淆(0.5)。「盐酸二甲双胍片」
/// 剥成「二甲双胍」是**推断**——原文并没有这四个字,上层要能把它和原样命中区分开。
const STRIPPED_CONFIDENCE: f32 = 0.8;

/// 药名规范化前的**确定性剥壳**:真实处方写「盐酸二甲双胍片」「阿托伐他汀钙片」,而
/// 字典按通用名(二甲双胍 / 阿托伐他汀)收录。这里对已 normalize 的词生成候选词干 ——
/// 去前导盐基、去尾部剂型、再去尾部成盐金属字 —— 供 [`normalize`] 在直配未命中时逐个
/// 重试。**候选式、不破坏原词**:某个候选没配上只是跳过,所以「碳酸氢钠」不会被误删成
/// 「碳酸氢」再乱配——两个候选都配不上就整体 miss(诚实,交给上层保留原文)。
fn drug_stem_candidates(norm: &str) -> Vec<String> {
    // 前导成盐酸根(仅去一次)。刻意不含「单硝酸/碳酸氢」这类本身就是药名一部分的。
    const SALT_PREFIX: &[&str] = &[
        "盐酸",
        "硫酸氢",
        "硫酸",
        "苯磺酸",
        "琥珀酸",
        "马来酸",
        "富马酸",
        "酒石酸",
        "氢溴酸",
        "磷酸",
        "醋酸",
        "枸橼酸",
        "甲磺酸",
        "门冬",
    ];
    const FORM_SUFFIX: &[&str] = &[
        "缓释片",
        "控释片",
        "分散片",
        "咀嚼片",
        "泡腾片",
        "肠溶片",
        "口服溶液",
        "口服液",
        "软胶囊",
        "缓释胶囊",
        "肠溶胶囊",
        "注射液",
        "混悬液",
        "干混悬剂",
        "颗粒",
        "胶囊",
        "滴丸",
        "胶丸",
        "散",
        "糖浆",
        "贴片",
        "栓",
        "片",
    ];
    // 尾部成盐(长的排前面:「琥珀酸钠」要先于「钠」被匹配)。
    const SALT_SUFFIX: &[&str] = &[
        "琥珀酸钠",
        "磷酸钠",
        "磺酸钠",
        "氨丁三醇",
        "钙",
        "钠",
        "钾",
        "镁",
    ];

    // 剥前缀:浓度(10%氯化钾注射液)与「注射用」(注射用泮托拉唑钠 → 泮托拉唑钠)。
    let no_pct = norm.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '%');
    let head = normalize_term("注射用");
    let stem0 = no_pct.strip_prefix(&head).unwrap_or(no_pct).to_string();

    // 反复去尾部剂型(「…钙片」先去片)。刻意先只去剂型、不去盐前缀:否则「琥珀酸亚铁片」
    // 会被剥成「亚铁」而丢掉本来能直配的「琥珀酸亚铁」。
    let mut form_stripped = stem0.clone();
    while let Some(f) = FORM_SUFFIX
        .iter()
        .find(|f| form_stripped.ends_with(&normalize_term(f)))
    {
        form_stripped.truncate(form_stripped.len() - normalize_term(f).len());
    }

    // 载液:「左氧氟沙星氯化钠注射液」= 药 + 载液,去剂型后再去载液即落到通用名。
    // 载液组合是无穷的(氯化钠/葡萄糖 × 每种药),所以剥壳处理,绝不给字典开条目。
    const CARRIER: &[&str] = &["氯化钠", "葡萄糖"];
    // 候选逐级放宽:①去剂型 ②再去载液 ③再去盐前缀 —— 每级再各出一个「去尾部成盐」的版本。
    let mut bases = vec![stem0, form_stripped.clone()];
    for c in CARRIER {
        if let Some(r) = form_stripped.strip_suffix(&normalize_term(c)) {
            if r.chars().count() > 1 {
                bases.push(r.to_string());
            }
        }
    }
    for p in SALT_PREFIX {
        let pn = normalize_term(p);
        if let Some(r) = form_stripped.strip_prefix(&pn) {
            bases.push(r.to_string());
            break;
        }
    }
    let mut cands: Vec<String> = Vec::new();
    for b in bases {
        if b.chars().count() > 1 && b != norm {
            cands.push(b.clone());
        }
        for s in SALT_SUFFIX {
            let sn = normalize_term(s);
            let Some(shorter) = b.strip_suffix(&sn) else {
                continue;
            };
            if shorter.chars().count() > 1 && shorter != norm {
                cands.push(shorter.to_string());
            }
            break; // 只剥一层成盐
        }
    }
    cands.dedup();
    cands
}

/// 去掉尾部剂量规格:「醋酸泼尼松片5mg」→「醋酸泼尼松片」。处方上药名后面常跟规格,
/// 它不是药名的一部分。返回 `None` 表示没有可去的规格。
fn strip_trailing_dose(s: &str) -> Option<String> {
    // 长的写法排前面,免得「万单位」先被「单位」吃掉。
    const DOSE_UNITS: &[&str] = &[
        "万单位",
        "万iu",
        "单位",
        "mcg",
        "mg",
        "ug",
        "gm",
        "ml",
        "iu",
        "g",
        "u",
        "%",
    ];
    for u in DOSE_UNITS {
        // 取原串末尾同样字符数的后缀 —— 从 s 自己的 char 里取,所以它的字节长度必然落在
        // UTF-8 边界上。**不能**拿 s.to_lowercase() 的字节偏移去切 s:小写化会改变字节长度
        // (「ẞ」3 字节 → 「ß」2 字节,「İ」2 → 3),切出来要么错位要么 panic。
        let n = u.chars().count();
        let tail: String = {
            let mut cs: Vec<char> = s.chars().rev().take(n).collect();
            cs.reverse();
            cs.into_iter().collect()
        };
        if tail.chars().count() < n || tail.to_lowercase() != *u {
            continue;
        }
        let head = &s[..s.len() - tail.len()];
        let stem = head.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
        // 必须真去掉了数字(否则「片g」这种误判),且剩余不能为空。
        if stem.len() < head.len() && !stem.is_empty() {
            return Some(stem.to_string());
        }
    }
    None
}

/// 术语名**候选拆分**(提取层调用,不在 [`normalize`] 里跑):真实报告/处方写
/// 「甘油三酯 TG」「肌酐 Cr(Scr)」「甲泼尼龙片(美卓乐)4mg」——整串精确查表必 miss,
/// 拆开后各自查即命中。
///
/// 产出候选:去括号后的主体 → 主体按空格/斜杠/顿号切出的 token → 括号内内容切出的
/// token(括号里常是同义缩写或商品名)→ 每个候选再去掉尾部剂量规格。**纯确定性,不是
/// 模型**。调用方按顺序拿每个候选去 [`normalize`](药名的盐基/剂型剥壳在那儿做),第一个
/// 命中即用;全不命中就是 miss(诚实,上层保留原文)。
///
/// [`normalize`] 本身**刻意不做**这件事:它是单词查表,拆分是提取层的职责(design §6)。
pub fn term_candidates(name: &str) -> Vec<String> {
    let is_open = |c: char| matches!(c, '(' | '（' | '[' | '【');
    let is_close = |c: char| matches!(c, ')' | '）' | ']' | '】');
    let (mut stripped, mut inner_all) = (String::new(), Vec::<String>::new());
    let mut inner = String::new();
    let mut depth = 0i32;
    for c in name.chars() {
        if is_open(c) {
            depth += 1;
        } else if is_close(c) {
            depth -= 1;
            if depth <= 0 && !inner.trim().is_empty() {
                inner_all.push(inner.trim().to_string());
                inner.clear();
            }
        } else if depth >= 1 {
            inner.push(c);
        } else {
            stripped.push(c);
        }
    }
    // 刻意**不按斜杠切**:化验名里的「/」多半是比值本身(尿白蛋白/肌酐比值、AST/ALT),
    // 切开会先命中分子(尿白蛋白)而丢掉真正的项(ACR)。
    const SEPS: [char; 5] = [' ', '\u{3000}', '、', ',', '，'];
    // OCR 常把右括号丢掉(「(肌酐」):残留的括号内内容也收进候选,否则整段被吞掉必 miss。
    if !inner.trim().is_empty() {
        inner_all.push(inner.trim().to_string());
    }
    // 候选顺序:原串 → 去括号主体 → 各自的 token → **括号内内容放最后兜底**。
    // 括号里的裸缩写常与别的项撞车(尿常规「尿红细胞计数(RBC)」的 RBC = 血 RBC;
    // 「血小板压积(PCT)」的 PCT = 降钙素原),优先用它会造成**误配**——比 miss 危险得多。
    let mut cands: Vec<String> = vec![name.trim().to_string(), stripped.trim().to_string()];
    for src in [name, &stripped] {
        for t in src.split(SEPS) {
            let t = t.trim();
            if !t.is_empty() {
                cands.push(t.to_string());
            }
        }
    }
    for blk in &inner_all {
        for t in blk.split(SEPS) {
            let t = t.trim();
            if !t.is_empty() {
                cands.push(t.to_string());
            }
        }
    }
    // 每个候选再补一个「去掉尾部规格」的版本(处方:醋酸泼尼松片5mg)。
    for i in 0..cands.len() {
        if let Some(stem) = strip_trailing_dose(&cands[i]) {
            cands.push(stem);
        }
    }
    cands.retain(|c| !c.is_empty());
    cands.dedup();
    cands
}

/// 单位**记法折叠**(纯记法,不碰语义):报告写 `×10^9/L`、`10E9/L`、`μmol/L`、全角字符,
/// 字典按 UCUM 写 `10*9/L`、`umol/L` —— 同一个单位的不同写法。字典行和报告单位都过这个
/// 函数再比较,记法差异就不再是 miss。
///
/// **刻意不折大小写**:`mU/L`(毫单位)≠ `MU/L`(兆单位),折了就差 6 个数量级。真正的
/// 量纲差异只能靠字典 `units[]` 里的显式换算,不能靠字符串猜。
pub fn normalize_unit(raw: &str) -> String {
    let mut s = String::with_capacity(raw.len());
    for ch in raw.chars() {
        let ch = to_halfwidth(ch);
        if ch.is_whitespace() {
            continue;
        }
        s.push(match ch {
            'µ' | 'μ' => 'u',
            '×' | '✕' => 'x',
            '^' => '*',
            // 上标数字(报告写 1.73m²、mm³)→ 普通数字。
            '²' => '2',
            '³' => '3',
            c => c,
        });
    }
    // 科学计数写法 10E9 / 10e9 → UCUM 10*9;再去掉前导乘号(×10*9/L → 10*9/L)。
    let s = s.replace("10E", "10*").replace("10e", "10*");
    s.strip_prefix(['x', 'X']).unwrap_or(&s).to_string()
}

/// Map a single candidate term to its canonical concept. Returns `None` on no
/// hit. This is a lookup, not a full-text scan — locating terms in free text is
/// the extraction layer's job (design §6).
///
/// An exact (normalized) alias hit yields `confidence == 1.0`; an
/// `ocr_confusions` hit yields `confidence == 0.5`. If a raw drug term
/// (`盐酸二甲双胍片`) doesn't match directly, deterministic salt/form stripping is
/// retried and, on a **drug** hit, returned at [`STRIPPED_CONFIDENCE`] (0.8) —— 剥壳是推断,
/// 不能和原样命中同等信任。
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
    // 药名剥壳兜底:去盐/剂型后重试,只在 drug 命名空间里查(仍确定性、可核对)。
    for cand in drug_stem_candidates(&norm) {
        if let Some(hit) = idx.drug_aliases.get(&cand) {
            return Some(idx.to_match(hit, STRIPPED_CONFIDENCE));
        }
    }
    None
}

/// 处方语境的 [`normalize`]:**只在 drug 命名空间里查**。同名跨类别时(「叶酸」「维生素C」
/// 「氯化钠」既是化验也是药),提取层解析的是处方就该用这个,否则会被化验项抢走。
/// 先整串直配,再走确定性剥壳(前缀「注射用」/ 盐基 / 剂型 / 尾部成盐)。
pub fn normalize_drug(raw_term: &str) -> Option<Match> {
    let norm = normalize_term(raw_term);
    if norm.is_empty() {
        return None;
    }
    let idx = index();
    if let Some(hit) = idx.drug_aliases.get(&norm) {
        return Some(idx.to_match(hit, 1.0));
    }
    if let Some(hit) = idx.drug_confusions.get(&norm) {
        return Some(idx.to_match(hit, 0.5));
    }
    drug_stem_candidates(&norm)
        .iter()
        .find_map(|c| idx.drug_aliases.get(c))
        .map(|hit| idx.to_match(hit, STRIPPED_CONFIDENCE))
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
    fn drug_candidates_handle_prescription_writing() {
        // 处方真实写法:规格、商品名括号、复方 —— 拆候选后再剥壳即命中。
        let hit = |name: &str| {
            term_candidates(name)
                .iter()
                .find_map(|c| normalize(c))
                .unwrap_or_else(|| panic!("no candidate hit for {name}"))
                .key
        };
        assert_eq!(hit("醋酸泼尼松片 5mg"), "prednisone");
        assert_eq!(hit("甲泼尼龙片(美卓乐)4mg"), "methylprednisolone");
        assert_eq!(hit("硫酸羟氯喹片(纷乐)0.1g"), "hydroxychloroquine");
        // 规格不会把药名本身吃掉:剥出的候选仍是完整通用名。
        assert!(term_candidates("醋酸泼尼松片5mg").contains(&"醋酸泼尼松片".to_string()));
    }

    #[test]
    fn carrier_strip_never_swaps_one_drug_for_another() {
        // 载液剥离的边界:带钠的葡萄糖氯化钠 ≠ 不带钠的葡萄糖注射液,复方氯化钠 ≠ 「复方」。
        // 剥壳只能在**整串查不到**时才放宽,顺序错了就是临床误配(病人多输/少输一份钠)。
        assert_eq!(
            normalize_drug("葡萄糖氯化钠注射液").unwrap().key,
            "glucose_sodium_chloride"
        );
        assert_eq!(
            normalize_drug("复方氯化钠注射液").unwrap().key,
            "compound_sodium_chloride"
        );
        // 真正的「药 + 载液」才剥到通用名。
        assert_eq!(
            normalize_drug("左氧氟沙星氯化钠注射液").unwrap().key,
            "levofloxacin"
        );
    }

    #[test]
    fn stripped_match_is_not_full_confidence() {
        // 剥壳是**推断**:原文写的是「盐酸二甲双胍片」,字典命中的是「二甲双胍」。
        // 上层必须能把它和原样精确命中区分开(否则 OCR 掉一个字导致的换药无从察觉)。
        assert_eq!(normalize("盐酸二甲双胍片").unwrap().confidence, 0.8);
        assert_eq!(normalize_drug("注射用泮托拉唑钠").unwrap().confidence, 0.8);
        // 原样命中仍是 1.0。
        assert_eq!(normalize("二甲双胍").unwrap().confidence, 1.0);
        assert_eq!(normalize_drug("二甲双胍").unwrap().confidence, 1.0);
    }

    #[test]
    fn fuzz_random_input_never_panics() {
        // 硬编码的敌意串只能挡住已知的坑;随机 fuzz 才挡得住下一个切片 bug。
        // 用固定种子的 xorshift(可复现,无外部依赖)。
        const ALPHABET: &[char] = &[
            'ẞ', 'İ', 'K', 'Ω', 'ǅ', 'µ', 'μ', '×', '^', '²', '％', 'Ａ', '（', '）', '(', ')',
            '[', ']', '、', '/', ' ', '\u{3000}', '\u{200b}', '\u{0301}', '🩺', '肌', '酐', '片',
            '钠', '注', '射', '用', '5', '0', '.', '%', 'm', 'g', 'L', 'u', '万', '单', '位',
        ];
        let mut state: u64 = 0x9E3779B97F4A7C15;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..20_000 {
            let len = (next() % 24) as usize;
            let s: String = (0..len)
                .map(|_| ALPHABET[(next() as usize) % ALPHABET.len()])
                .collect();
            let _ = normalize(&s);
            let _ = normalize_drug(&s);
            let _ = normalize_unit(&s);
            for c in term_candidates(&s) {
                let _ = normalize(&c);
                let _ = normalize_drug(&c);
            }
        }
    }

    #[test]
    fn hostile_input_never_panics() {
        // 输入来自 OCR 出来的病历文本 = 不可信。任何输入都只能 miss,不能 panic(panic = DoS)。
        // 「ẞ5mg」曾真的崩过:小写化改变字节长度(ẞ 3 字节 → ß 2 字节),按 lowercase 的偏移
        // 切原串会切在 UTF-8 字符中间。
        let hostile = [
            "ẞ5mg",
            "İ5mg",
            "ẞẞ5mg",
            "İ1g",
            "ǅ5ml",
            "5mg",
            "%",
            "0.5",
            "mg",
            "万单位",
            "(((",
            ")))",
            "（（（",
            "()",
            "（）",
            "、、、",
            "   ",
            "",
            "\u{200b}",
            "🩺5mg",
            "肌酐\u{0301}",
            "10%",
            "%%%",
            "注射用",
            "注射用片",
            "钠",
            "片",
        ];
        for h in hostile {
            let _ = normalize(h);
            let _ = normalize_drug(h);
            let _ = normalize_unit(h);
            for c in term_candidates(h) {
                let _ = normalize(&c);
                let _ = normalize_drug(&c);
            }
        }
        // 超长串也不能崩(也别指数爆炸)。
        let long_paren = "(".repeat(5_000) + &"肌酐 5mg ".repeat(2_000);
        let _ = term_candidates(&long_paren);
        let _ = normalize(&long_paren);
    }

    #[test]
    fn strips_salt_and_dosage_form_to_ingredient() {
        // 真实处方写法(盐基 + 通用名 + 剂型)→ 剥壳后命中通用名。
        assert_eq!(normalize("盐酸二甲双胍片").unwrap().key, "metformin");
        assert_eq!(normalize("琥珀酸美托洛尔缓释片").unwrap().key, "metoprolol");
        assert_eq!(normalize("苯磺酸氨氯地平片").unwrap().key, "amlodipine");
        // 「…钙片」先去剂型再去成盐金属字。
        assert_eq!(normalize("阿托伐他汀钙片").unwrap().key, "atorvastatin");
        // 剥壳到通用名(碳酸氢钠已收录):去剂型「片」后命中,不会再去剥成「碳酸氢」。
        assert_eq!(normalize("碳酸氢钠片").unwrap().key, "sodium_bicarbonate");
        // 候选式不破坏:词典没有的整体 miss,绝不误配(碳酸氢钙 → 碳酸氢 也配不上 → None)。
        assert!(normalize("碳酸氢钙片").is_none());
        // 剥壳只接受 Drug 类,不会把化验名误当药。
        assert_eq!(normalize("阿托伐他汀").unwrap().category, Category::Drug);
    }

    #[test]
    fn term_candidates_split_composite_names() {
        // 提取层拆分:整串必 miss,拆出的 token 命中。
        let hit = |name: &str| {
            term_candidates(name)
                .iter()
                .find_map(|c| normalize(c))
                .unwrap_or_else(|| panic!("no candidate hit for {name}"))
                .key
        };
        assert!(normalize("甘油三酯 TG").is_none(), "整串不该直配");
        assert_eq!(hit("甘油三酯 TG"), "triglycerides");
        assert_eq!(hit("肌酐 Cr(Scr)"), "creatinine");
        assert_eq!(hit("白细胞计数(WBC)"), "wbc");
        // 括号里才是能查到的那个(主体查不到时用括号内缩写)。
        assert_eq!(hit("糖化血红蛋白(HbA1c)"), "hba1c");
        // 拆不出任何已知 token → 老老实实 miss。
        assert!(term_candidates("完全不是术语 XYZ")
            .iter()
            .all(|c| normalize(c).is_none()));
    }

    #[test]
    fn unit_notation_folds_but_never_folds_case() {
        // 记法差异(报告 vs UCUM)折叠后必须一致。
        for (report, ucum) in [
            ("×10^9/L", "10*9/L"),
            ("10^12/L", "10*12/L"),
            ("10E9/L", "10*9/L"),
            ("μmol/L", "umol/L"),
            ("µmol/L", "umol/L"),
            ("ｍｇ/ｄＬ", "mg/dL"),
            ("mg / L", "mg/L"),
            ("mL/min/1.73m²", "mL/min/1.73m2"),
        ] {
            assert_eq!(
                normalize_unit(report),
                normalize_unit(ucum),
                "{report} 应折叠到 {ucum}"
            );
        }
        // 大小写**不折**:mU/L(毫)≠ MU/L(兆),折了差 6 个数量级。
        assert_ne!(normalize_unit("mU/L"), normalize_unit("MU/L"));
        // 量纲不同的单位不会被记法折叠糊到一起(那是 units[] 换算的活)。
        assert_ne!(normalize_unit("mg/dL"), normalize_unit("mmol/L"));
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
        // Coverage expansion (2026-07-14.1): 191 + 446 按专科批次扩容 = 637
        // (血液/凝血/铁代谢、生化、内分泌/骨代谢、心肌/感染、风湿/肿标、尿粪、西药、抗感染+中成药)。
        // A drift here means an entry was accidentally dropped or duplicated.
        assert_eq!(
            dictionary_entries().len(),
            637,
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
    fn no_duplicate_alias_within_category() {
        // Fail loudly rather than silently shadow: 同一类别内,一个归一别名不能被两个条目
        // 认领。跨类别同名是**允许的**(「叶酸」既是化验也是药),由 normalize/normalize_drug
        // 的两张表分流 —— 见 build_index。
        let mut seen: HashMap<(bool, String), String> = HashMap::new();
        for e in dictionary_entries() {
            let is_drug = e.category == Category::Drug;
            for a in e.aliases.iter().chain(e.ocr_confusions.iter()) {
                let k = (is_drug, normalize_term(a));
                if let Some(prev) = seen.get(&k) {
                    assert_eq!(
                        prev, &e.key,
                        "duplicate normalized alias {a:?} in entries {prev} and {}",
                        e.key
                    );
                }
                seen.insert(k, e.key.clone());
            }
        }
    }

    #[test]
    fn drug_namespace_wins_in_prescription_context() {
        // 「叶酸」在化验单上是化验项,在处方上是药 —— 两张表各查各的,谁也不抢谁。
        let lab = normalize("叶酸").expect("化验单语境");
        assert_eq!(lab.category, Category::Lab);
        let drug = normalize_drug("叶酸片 5mg")
            .or_else(|| {
                term_candidates("叶酸片 5mg")
                    .iter()
                    .find_map(|c| normalize_drug(c))
            })
            .expect("处方语境");
        assert_eq!(drug.category, Category::Drug);
        // 处方专表查不到的词照样 miss,不会退回化验项。
        assert!(normalize_drug("肌酐").is_none());
    }

    #[test]
    fn strips_injection_prefix_and_salt_suffix() {
        // 「注射用 + 通用名 + 成盐」:剥前缀与尾部成盐后命中通用名。
        assert_eq!(
            normalize_drug("注射用泮托拉唑钠").unwrap().key,
            "pantoprazole"
        );
        // 只剥剂型的候选必须先于「剥盐前缀」被试,否则「琥珀酸亚铁」会被剥成「亚铁」而丢掉。
        assert_eq!(
            normalize_drug("琥珀酸亚铁片").unwrap().canonical_name,
            "琥珀酸亚铁"
        );
    }
}
