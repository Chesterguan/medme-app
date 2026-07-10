# Terminology Normalization Layer — Design (Issue #18)

> **一句话:** 一个明确、可不断扩展、**不运行但不出错**的临床术语归一化映射层。把中文/英文/缩写/OCR 拆字的 raw 术语,映射到内部规范键 + 规范中文名 + 国际编码(LOINC/RxNorm/ATC/OMOP)+ canonical 单位 + 显式换算。

关联:[docs/015_Trends_and_Timelines.md](../../015_Trends_and_Timelines.md) §3(本 spec 是其「归一化词典/抽取」子集的独立落地)。Issue: https://github.com/Chesterguan/medme/issues/18

---

## 1. 范围

**做:** 可复用的映射层 —— 一份版本化静态词典(JSON)+ 一个查表函数 `normalize()` + 测试。

**不做(明确边界):**
- 不碰 event log / DB schema / observations 表 / pipeline 接入 / UI(那些是 015 后续)。
- 不写运行时换算/校验代码(此层没有「值」)。**但换算方法必须在数据里写死写对**,任何人照着 `units[]` 就能算,零歧义。
- `source_document_id / source_location` 不进词典 —— 每次命中由调用方(pipeline)附加。

**为什么现在做:** 标准(编码 + canonical 单位 + 换算)是概念身份,必须第一天立对;词典是 shipped 资源,日后 keyed 数据都跟它走,晚立就得迁移。词汇 mapping 对医生/科研价值大(可比、可互操作),对普通用户价值小 —— 接入流程延后。

**覆盖(V1):** docs/015 §3.5 全清单 —— ~25 化验项 + 4 类生命体征 + ~30 常见慢病药。

## 2. 架构

新建 crate `packages/terminology`(parser 管抽取,terminology 是可复用词典,边界干净)。

```
packages/terminology/
  src/lib.rs        // normalize() + 类型 + alias 索引(OnceLock) + 测试
  dictionary.json   // 版本化静态词典(include_str! 编进二进制)
```

代码极简,丰富度全在**数据**里 —— 标准数据齐全,代码不膨胀。

## 3. 词典条目 schema

统一条目,`category` 区分 `lab | vital | drug`。

### lab / vital 条目
```jsonc
{
  "key": "creatinine",
  "canonical_name": "肌酐",
  "category": "lab",                                  // lab | vital
  "system": "serum/plasma",                           // LOINC specimen,防 血肌酐/尿肌酐 collapse
  "codes": { "loinc": "14682-9", "omop_concept_id": 3020564 },
  "canonical_unit": "umol/L",                          // UCUM 记法
  "units": [                                           // 显式换算,见 §4
    { "unit": "umol/L", "slope": 1.0,   "intercept": 0 },
    { "unit": "mg/dL",  "slope": 88.42, "intercept": 0 }
  ],
  "aliases": ["肌酐","血肌酐","血清肌酐","Cr","CREA","CRE","Creatinine","SCr"],
  "ocr_confusions": ["肌研","肌肝"]                     // 命中→降置信,标可疑
}
```

### drug 条目
```jsonc
{
  "key": "metformin",
  "canonical_name": "二甲双胍",
  "category": "drug",
  "ingredient": "Metformin",
  "codes": { "rxnorm": "6809", "atc": "A10BA02", "omop_concept_id": null },
  "aliases": ["二甲双胍","盐酸二甲双胍","格华止","美迪康","Metformin","二甲双胍缓释片","甲福明"],
  "ocr_confusions": []
}
```
药无 `canonical_unit / units / system`(剂量换算是 015 抽取层的事)。

## 4. 换算:显式、不歧义、可扩展

**统一形式**(数据里写死,任何消费者照此计算):
```
canonical_value = slope × source_value + intercept
```
- 线性(绝大多数):`intercept = 0`。例:肌酐 mg/dL → `slope=88.42, intercept=0`。
- 仿射:HbA1c IFCC mmol/mol → NGSP % → `slope=0.09148, intercept=2.152`。
- canonical 单位本身:`slope=1, intercept=0`。

每个 analyte 把**所有可接受源单位**各列一行 `(unit, slope, intercept)`。加单位 = 加一行。

> 关键正确性:只留 `factor` 会把 HbA1c 等仿射换算算错 —— 所以统一用 slope/intercept。

## 5. 编码来源:OMOP vocab(correct-by-construction,不手猜)

本地 OMOP testbed:`postgresql://postgres@localhost:5435/mimiciv_omop`,`SET search_path=omop_cdm`(原生 PG15,无密码,含全 OMOP vocab)。

每个 analyte/drug 从 vocab 查:
- OMOP standard `concept_id`(`standard_concept='S'`)。
- **与 canonical 单位一致的** LOINC/RxNorm/ATC 码。
  - 反例警示:LOINC `2160-0` 是 Creatinine **[Mass/volume]**(→mg/dL);µmol/L(molar)对应 `14682-9`(concept 3020564)。canonical 选 µmol/L 就必须配 molar 码,否则编码/单位打架。
- UCUM canonical 单位(UCUM 是 OMOP/FHIR 标准单位词表)。

查询范式:
```sql
SET search_path=omop_cdm;
SELECT concept_id, concept_name, concept_code, standard_concept
FROM concept WHERE vocabulary_id='LOINC' AND concept_code = :code;
```

## 6. API

```rust
pub struct Match {
    pub key: String,               // 内部规范键
    pub canonical_name: String,    // 规范中文名
    pub category: Category,        // Lab | Vital | Drug
    pub codes: Codes,              // loinc / rxnorm / atc / omop_concept_id (各 Option)
    pub ingredient: Option<String>,// 仅 drug
    pub matched_alias: String,     // 命中的原别名(可溯源)
    pub confidence: f32,           // 别名精确命中 1.0;ocr_confusions 命中 0.5
}

/// 单个候选术语 → 规范映射。无命中返回 None。不做全文扫描(那是抽取层)。
pub fn normalize(raw_term: &str) -> Option<Match>;
```

**匹配前归一化**(建索引与查询都过同一函数):小写化 + 全角→半角 + 去内部空白(命中 OCR 拆字「肌 酐」)。加载时用 `OnceLock` 建 `normalized_alias → entry` 索引;`ocr_confusions` 建独立低置信索引。

**信息不丢的四条红线:**
1. mapping 永远**附加**,绝不替换 raw_term —— 调用方保留原文 + span。
2. `system` 字段保留标本,serum ≠ urine 不 collapse。
3. LOINC 选与 canonical 单位一致的 property/scale(§5)。
4. `codes` 是多码 map,不把多概念塞进一个字段。

## 7. 测试(与代码同 crate)

- 别名精确命中:「谷丙转氨酶 / ALT / GPT / SGPT」→ 同一 `alt`;「肌酐 / 血肌酐 / Cr / SCr」→ `creatinine`。
- 归一化:全角「ＡＬＴ」、大小写「crea」、OCR 拆字「肌 酐」均命中。
- `ocr_confusions`(「肌研」)→ 命中但 confidence=0.5。
- 未命中 → `None`。
- 换算正确性:肌酐 mg/dL slope=88.42;HbA1c mmol/mol 仿射 slope/intercept 存在且 ≠ 简单 factor。
- 词典完整性:§3.5 每个清单项都在词典里(防漏);每条 lab/vital 都有 `canonical_unit` 且 `units[]` 含 canonical 自身行(slope=1)。
- 编码一致性:每条 lab 的 loinc 非空 → canonical_unit 与该 LOINC 的 property 不矛盾(molar↔µmol/mmol,mass↔mg/g)。

## 8. 交付

- 实现由 sonnet/opus subagent 完成;编码/单位在实现时连 OMOP testbed 查全查对。
- 别名整理是主体工作量:§3.5 每项手工整理(中文全称/简称 + 英文 + 缩写 + OCR 拆字)。
- 门禁:`cargo fmt` + `clippy` + `test` 全绿(CI 同款)后再交。
