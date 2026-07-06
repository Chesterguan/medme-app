# 003 · Core Data Model · 核心数据模型

> **这是共享契约。** parser / ocr / ai / timeline / search / export 全部对着本文写代码。
> 改这里 = 改所有人。任何变更必须走 migration + 版本号。

关联:[002_Architecture](002_Architecture.md) · [004_Import_Pipeline](004_Import_Pipeline.md) · [ADR/0004](ADR/0004-core-model-independence.md)

---

## 1. 分层与阶段

- **L1(Raw)**:`source_file` —— 不可变原文件登记。
- **L2(Clinical Core Model)**:`document` / `ocr_result` / `clinical_event`。
- **FTS**:`document_fts` —— 派生索引,可重建。
- 表按阶段标注:**[v0.1]** 现在建;**[v0.2+]** 现在定义、后续启用(先建空表,减少后续 migration 冲击)。

设计约束(见 [ADR/0004](ADR/0004-core-model-independence.md)):**不引入 FHIR 资源结构,不引入 OMOP 宽表**。L2 只存 `code_system + code + display`,术语映射留给 L3。

---

## 2. SQLite Schema

```sql
-- schema 版本;每次结构变更 +1,写入迁移历史
PRAGMA user_version = 1;

-- ── L1 · Raw Storage ──────────────────────────────────────────────
-- 不可变。原文件按 sha256 存入 CAS,这里只登记元数据。
CREATE TABLE source_file (
    id            INTEGER PRIMARY KEY,
    content_hash  TEXT    NOT NULL UNIQUE,   -- sha256(hex),CAS 主键 & 去重键
    original_name TEXT    NOT NULL,          -- 用户导入时的文件名
    mime_type     TEXT    NOT NULL,          -- 探测得到,如 application/pdf
    byte_size     INTEGER NOT NULL,
    storage_path  TEXT    NOT NULL,          -- CAS 相对路径,由 hash 派生
    imported_at   TEXT    NOT NULL           -- RFC3339 UTC
);

-- ── L2 · Document ─────────────────────────────────────────────────
-- 一份逻辑医疗文档。一个 source_file 可产出 1+ document(v0.1 默认 1:1)。
CREATE TABLE document (
    id            INTEGER PRIMARY KEY,
    source_file_id INTEGER NOT NULL REFERENCES source_file(id),
    doc_type      TEXT    NOT NULL DEFAULT 'unknown',  -- 见 §3 枚举
    doc_date      TEXT,                     -- 文档上的临床日期(RFC3339,可空);时间线主轴
    title         TEXT,
    language      TEXT,                     -- 'zh' / 'en' / 'mixed' / null
    page_count    INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT    NOT NULL,
    UNIQUE(source_file_id)                  -- v0.1 约束:1 文件 1 文档;v0.2 放开
);
CREATE INDEX idx_document_date ON document(doc_date);
CREATE INDEX idx_document_type ON document(doc_type);

-- ── L2 · OCR Result ───────────────────────────────────────────────
-- OCR 派生文本。逐页一行。可溯源:记录 backend + 模型版本。可重建。
CREATE TABLE ocr_result (
    id            INTEGER PRIMARY KEY,
    document_id   INTEGER NOT NULL REFERENCES document(id) ON DELETE CASCADE,
    page_no       INTEGER NOT NULL,         -- 从 1 起
    backend       TEXT    NOT NULL,         -- 'native' | 'onnx' | 'vlm'
    model_version TEXT    NOT NULL,         -- 如 'ppocr-v5' / 'paddleocr-vl-0.9b'
    text          TEXT    NOT NULL,
    confidence    REAL,                     -- 0..1,页级平均
    layout_json   TEXT,                     -- 版式/表格结构(可空),见 §4
    created_at    TEXT    NOT NULL,
    UNIQUE(document_id, page_no)
);

-- ── FTS · 全文索引(派生,可重建)────────────────────────────────
-- content-less external-content FTS,内容取自 ocr_result.text + document.title
CREATE VIRTUAL TABLE document_fts USING fts5(
    title,
    body,
    document_id UNINDEXED,
    tokenize = 'unicode61 remove_diacritics 2'   -- 中英通用;中文见 §5
);

-- ── L2 · Clinical Event ── [v0.2+] ────────────────────────────────
-- 归一化临床事件 = Clinical Core Model 的心脏。v0.1 建空表,v0.2 起写入。
CREATE TABLE clinical_event (
    id            INTEGER PRIMARY KEY,
    document_id   INTEGER NOT NULL REFERENCES document(id) ON DELETE CASCADE,
    event_type    TEXT    NOT NULL,   -- diagnosis|lab_result|medication|procedure|imaging|allergy|vital_sign
    event_date    TEXT,               -- RFC3339,时间线精细轴
    code_system   TEXT,               -- 'ICD-10'|'SNOMED'|'LOINC'|'RxNorm'|... (L3 映射入口)
    code          TEXT,
    display       TEXT,               -- 原文/人读名称
    value_num     REAL,               -- 化验/生命体征数值
    value_unit    TEXT,
    value_text    TEXT,               -- 定性结果 / 自由文本
    confidence    REAL,               -- 抽取置信度
    -- ★ 可溯源:指回 ocr_result 中的字符区间(原则 4)
    source_ocr_id INTEGER REFERENCES ocr_result(id),
    source_span   TEXT,               -- JSON: {start, end}(字符偏移)
    created_at    TEXT    NOT NULL
);
CREATE INDEX idx_event_date ON clinical_event(event_date);
CREATE INDEX idx_event_type ON clinical_event(event_type);
CREATE INDEX idx_event_code ON clinical_event(code_system, code);

-- ── Import Batch(可选,便于回滚/审计)────────────────────────────
CREATE TABLE import_batch (
    id          INTEGER PRIMARY KEY,
    source      TEXT NOT NULL,   -- 'drag' | 'folder' | 'watch' | 'camera' | 'scan'
    started_at  TEXT NOT NULL,
    file_count  INTEGER NOT NULL DEFAULT 0
);
```

### FTS 同步触发器

```sql
CREATE TRIGGER ocr_ai AFTER INSERT ON ocr_result BEGIN
  INSERT INTO document_fts(document_id, title, body)
  VALUES (new.document_id,
          (SELECT title FROM document WHERE id = new.document_id),
          new.text);
END;
-- 删除/更新触发器同理(略);FTS 可整体 rebuild,不是唯一真相。
```

---

## 3. 枚举(值集)

| 字段 | 允许值 |
|---|---|
| `document.doc_type` | `lab_report` · `imaging_report` · `discharge_summary` · `prescription` · `clinical_note` · `pathology` · `other` · `unknown` |
| `ocr_result.backend` | `native`(文本层/DOCX,无 OCR)· `onnx` · `vlm` |
| `clinical_event.event_type` | `diagnosis` · `lab_result` · `medication` · `procedure` · `imaging` · `allergy` · `vital_sign` |
| `clinical_event.code_system` | `ICD-10` · `SNOMED` · `LOINC` · `RxNorm` · `OMOP` · `local` · `null` |
| `import_batch.source` | `drag` · `folder` · `watch` · `camera` · `scan` |

枚举以字符串存储(可读、可长期打开),在 Rust 侧用 enum 约束。

---

## 4. Rust 类型(core-model 导出)

> TS 侧从这些类型生成(如 `ts-rs`),`ui` 只依赖生成的类型。

```rust
// 时间用字符串(RFC3339)存,读出解析为 chrono::DateTime<Utc>;
// 存字符串保证 SQLite 文件长期自解释(原则 6)。

pub struct SourceFile {
    pub id: i64,
    pub content_hash: String,   // sha256 hex
    pub original_name: String,
    pub mime_type: String,
    pub byte_size: i64,
    pub storage_path: String,
    pub imported_at: DateTime<Utc>,
}

pub enum DocType { LabReport, ImagingReport, DischargeSummary,
                   Prescription, ClinicalNote, Pathology, Other, Unknown }

pub struct Document {
    pub id: i64,
    pub source_file_id: i64,
    pub doc_type: DocType,
    pub doc_date: Option<DateTime<Utc>>,
    pub title: Option<String>,
    pub language: Option<String>,
    pub page_count: i32,
    pub created_at: DateTime<Utc>,
}

pub enum OcrBackendKind { Native, Onnx, Vlm }

pub struct OcrResult {
    pub id: i64,
    pub document_id: i64,
    pub page_no: i32,
    pub backend: OcrBackendKind,
    pub model_version: String,
    pub text: String,
    pub confidence: Option<f32>,
    pub layout: Option<Layout>,   // 序列化进 layout_json
    pub created_at: DateTime<Utc>,
}

// 版式/表格结构;onnx 后端能给出 bbox+表格,vlm 更全
pub struct Layout {
    pub blocks: Vec<LayoutBlock>,      // 段落/标题/表格区块
    pub tables: Vec<Table>,            // 结构化表格(化验单关键)
}
pub struct LayoutBlock { pub kind: String, pub bbox: [f32; 4], pub text: String }
pub struct Table { pub bbox: [f32; 4], pub rows: Vec<Vec<String>> }

// [v0.2+]
pub enum EventType { Diagnosis, LabResult, Medication, Procedure, Imaging, Allergy, VitalSign }
pub struct ClinicalEvent {
    pub id: i64,
    pub document_id: i64,
    pub event_type: EventType,
    pub event_date: Option<DateTime<Utc>>,
    pub code_system: Option<String>,
    pub code: Option<String>,
    pub display: Option<String>,
    pub value_num: Option<f64>,
    pub value_unit: Option<String>,
    pub value_text: Option<String>,
    pub confidence: Option<f32>,
    pub source_ocr_id: Option<i64>,
    pub source_span: Option<Span>,   // {start,end} 字符偏移
    pub created_at: DateTime<Utc>,
}
pub struct Span { pub start: usize, pub end: usize }
```

---

## 5. 中文全文检索(FTS5)

SQLite `unicode61` 不做中文分词(按码点切,连续汉字成一个 token)。两种策略:

- **v0.1(默认,省力)**:入库前用轻量分词(如 `jieba-rs`)把中文文本切成空格分隔的词写入 `body`;英文天然按空格。查询侧同样分词。够用,零外部依赖(纯 Rust crate)。
- **升级路径**:需要更精准时换 FTS5 的 `trigram` tokenizer(子串匹配、无需分词,但索引更大),或引入自定义 tokenizer。

> ponytail: 先用 `jieba-rs` 预分词 + `unicode61`;子串/精度不够再上 `trigram`。别一上来自定义 tokenizer。

---

## 6. 迁移与长期兼容(原则 6)

- `PRAGMA user_version` 记 schema 版本;`core-model` 启动时按版本号顺序跑 migration。
- 迁移只做**加法**(加表/加列/加索引);删除/改名需保留兼容视图。
- CAS 布局也有版本(见 [004](004_Import_Pipeline.md) §布局)。
- 派生数据(`ocr_result` / `document_fts` / `clinical_event`)在最坏情况下**都能从 L1 原文件重建**——这是"Raw Never Dies"给的底气。

---

## 7. 一个约束检查(必须有)

`core-model` 至少留一个自检:插入同一文件两次 → 第二次因 `content_hash UNIQUE` 命中已存在记录、**不重复写 CAS、不新建 source_file**,返回既有 id。这是去重与"原始永存"的最小可跑验证。
