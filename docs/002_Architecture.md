# 002 · Architecture · 架构

关联:[001_PRD](001_PRD.md) · [003_Core_Data_Model](003_Core_Data_Model.md) · [004_Import_Pipeline](004_Import_Pipeline.md)

---

## 1. 总览

```
Raw Documents
      │
      ▼
Parser (类型/语言/旋转/版式)
      │
      ▼
OCR Pipeline (onnx 默认 / vlm 可选)
      │
      ▼
Clinical Core Model  ←─ 四层数据架构的 Layer 2
      │
      ├──► Timeline
      ├──► Search (FTS5)
      ├──► AI Summary / Extraction (v0.2+)
      └──► FHIR Export (v0.3+)
```

**并行的前提**:所有下游功能都对着 Layer 2 的**共享契约**(schema + 类型 + 包接口)写代码。契约先钉死(见 [003](003_Core_Data_Model.md)),四条线才能各干各的。

---

## 2. 四层数据架构

| 层 | 名称 | 职责 | 备注 |
|---|---|---|---|
| **L1** | Raw Storage | 存原始文件(图片/PDF/DICOM…) | **不可变**,内容寻址 CAS,见 [004](004_Import_Pipeline.md) |
| **L2** | Clinical Core Model | 归一化医疗数据 | **独立于 FHIR / OMOP 表**,为本地应用优化,见 [003](003_Core_Data_Model.md) |
| **L3** | Vocabulary | 术语映射(ICD-10 / SNOMED / LOINC / OMOP / 药典) | v0.3+;L2 只存 `code_system`+`code`,映射查这里 |
| **L4** | Application | 时间线 · 搜索 · AI · 导出 · 共享 | 全部构建在 L2 之上 |

**关键约束**:L2 是唯一的"真相层"。FHIR 与 OMOP 是**导出/映射目标**,不是内部存储格式——避免供应商锁定,也避免 FHIR 的复杂度渗进核心。

---

## 3. Monorepo 结构

```
medme/
├── apps/
│   ├── desktop/          # Tauri 外壳:窗口、命令、把各 package 接起来
│   └── website/          # 营销站(later)
├── packages/
│   ├── core-model/       # ★ 共享契约:schema、migrations、Rust 类型、DB 访问
│   ├── parser/           # 文件类型探测、PDF/DOCX/图像解码 → pages
│   ├── ocr/              # OcrBackend trait + onnx / vlm 实现
│   ├── ai/               # 抽取、总结(v0.2+),依赖 core-model + ocr
│   ├── connectors/       # 导入源(folder/watch/camera)+ 插件 SDK(v0.3+)
│   ├── timeline/         # 从 core-model 构建时间线
│   └── ui/               # React 组件(UI 设计由用户提供)
├── docs/
├── examples/
└── ADR/  → docs/ADR/
```

### 依赖方向(单向,禁止环)
```
core-model  ◄── parser
core-model  ◄── ocr        (ocr 也可用 parser 的 page 输出)
core-model  ◄── ai         ◄── ocr
core-model  ◄── timeline
core-model  ◄── connectors
ui          ── 只依赖类型,不依赖实现
apps/desktop ── 依赖以上全部,负责组装
```
`core-model` 不依赖任何其他 package(它是叶子/根)。

---

## 4. 语言与进程边界

- **核心逻辑全在 Rust**:parser / ocr(onnx)/ core-model / timeline / connectors。进程内,无 Python sidecar(见 [ADR/0002](ADR/0002-ocr-backend.md))。
- **VLM 后端**是可选外部进程:经 Ollama HTTP 或 `deepseek-ocr.rs`,仅难件/抽取时调用。默认不启用。
- **前端 React/TS** 通过 Tauri command(IPC)调用 Rust;不直接碰 DB 或文件系统。
- **UI 只依赖类型定义**(TS 侧从 Rust 类型生成),不含业务逻辑。

---

## 5. 技术栈

| 领域 | 选型 |
|---|---|
| 桌面 | Rust · Tauri · React · TypeScript |
| 数据库 | SQLite(单文件,长期可打开) |
| 搜索 | SQLite FTS5 |
| OCR(默认) | Rust 原生 ONNX + PP-OCRv5(`oar-ocr`) |
| OCR(可选/难件) | VLM:`PaddleOCR-VL` / `dots.ocr`,经 Ollama 或 `deepseek-ocr.rs` |
| 存储 | 本地文件系统 + 内容寻址(sha256) |
| 同步(future) | WebDAV · iCloud · OneDrive · NAS · S3-compatible |

---

## 6. 数据流(v0.1 端到端)

1. 用户拖入文件 → `connectors` 收集 → `core-model` 计算 sha256、写入 CAS、插 `source_file`。
2. `parser` 判类型/语言/旋转,PDF/DOCX/图像 → 逐页位图。
3. `ocr` 用 onnx 后端逐页出文本(含表格结构)→ 写 `ocr_result`,触发 `document_fts` 索引。
4. `core-model` 建/更新 `document`(含 `doc_date`)。
5. L4:`timeline` 按日期查询;搜索走 FTS5;查看器直接读 CAS 原件。

抽取(`clinical_event`)、FHIR 导出、术语映射在 v0.2+ 接入同一条流,不改动 L1/L2 已有结构。
