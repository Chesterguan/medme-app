# 007 · UI Guidelines · 界面准则

> UI **视觉设计由用户提供**。本文只定"有哪些视图、各视图靠什么数据、交互底线",供设计接入。视图是一等公民,必须做到位。

关联:[001_PRD](001_PRD.md) · [003_Core_Data_Model](003_Core_Data_Model.md) · [002_Architecture](002_Architecture.md)(UI 只依赖类型,经 Tauri command 取数)

---

## 1. v0.1 视图清单

| 视图 | 作用 | 数据来源 | 关键交互 |
|---|---|---|---|
| **Import · 导入** | 拖入/选文件夹,看导入进度与去重结果 | `import_batch` / `source_file` | 拖放;显示"新增 N,已存在 M(去重)" |
| **Timeline · 时间线** | 文档按 `doc_date` 时间轴排列 | `document` | 按年/月分组;未定日期单列一组;点击 → 文档查看器 |
| **Search · 搜索** | 关键词全文检索 | `document_fts` | 输入即搜;命中片段高亮;结果点开 → 查看器并定位 |
| **Document Viewer · 查看器** | 看原件 + OCR 文本对照 | CAS 原文件 + `ocr_result` | 原图/PDF 与文本左右对照;这是"溯源"的落点 |
| **Settings · 设置** | vault 路径、OCR 后端开关、导出 | 配置 | 开启 VLM 后端;导出整库 |

> v0.2+ 追加:结构化记录视图(化验趋势图、用药清单)、实体高亮(在查看器里标出抽取来源 `source_span`)。

---

## 2. 交互底线(必须满足,不可简化)

1. **原件永远可达。** 任何视图里的任何条目,一键跳到原始文件。派生数据(OCR/抽取)旁必须能看到"来源"。(原则 3、4)
2. **溯源可视。** 查看器里,OCR 文本与原图对照;v0.2 起抽取项点击 → 原文高亮 `source_span`。
3. **去重透明。** 导入时明确告知哪些被去重,不静默丢弃。
4. **无网络也全可用。** v0.1 全部视图离线工作;任何联网动作(云模型/同步)显式提示。
5. **可访问性基线。** 键盘可达、对比度达标、字号可放大(目标用户含老年人)。**不可砍。**
6. **数据可带走。** 设置里"导出整库"始终可用(随时可离开)。

---

## 3. 数据契约(UI ↔ 后端)

- UI **不碰** SQLite / 文件系统,一律经 Tauri command(IPC)。
- 命令返回 [003](003_Core_Data_Model.md) 定义的类型(TS 从 Rust 类型生成,单一真相)。
- 建议命令集(v0.1):`import_files` · `list_timeline` · `search(query)` · `get_document(id)` · `get_source_file_bytes(id)` · `export_vault`。

> ponytail: UI 只是 L2 上的视图层,不放业务逻辑。所有"怎么算"在 Rust,UI 只"怎么显示"。
