# 004 · Import Pipeline · 导入管线

关联:[002_Architecture](002_Architecture.md) · [003_Core_Data_Model](003_Core_Data_Model.md) · [ADR/0002](ADR/0002-ocr-backend.md) · [ADR/0003](ADR/0003-content-addressable-storage.md)

---

## 1. 管线总览

```
文件进入
   │  (connectors: drag / folder / watch / camera / scan)
   ▼
① 计算 sha256  ──► 已存在? ──是──► 返回既有 source_file(去重,不重复存)
   │否
   ▼
② 写入 CAS(不可变)+ 插 source_file
   │
   ▼
③ parser:探测 mime/语言/旋转;PDF/DOCX/图像 → 逐页位图
   │
   ▼
④ ocr:OcrBackend 逐页 → text + layout → 插 ocr_result → FTS 触发
   │
   ▼
⑤ core-model:建/更新 document(doc_type 猜测、doc_date 抽取、page_count)
   │
   ▼
完成 → 时间线 / 搜索立即可见
```

每一步失败**只影响该文件**,不回滚整批;原文件(①②)一旦落地即永存,后续步骤可随时重跑。

---

## 2. 内容寻址存储(CAS)

见 [ADR/0003](ADR/0003-content-addressable-storage.md)。

- 主键 = `sha256(文件字节)` 的十六进制。
- **布局**(v1):`<vault>/objects/<hash[0:2]>/<hash[2:4]>/<hash>`。两级前缀分桶,避免单目录爆炸。
- **不可变**:写入用"临时文件 + 原子 rename";已存在则跳过。永不覆盖、永不自动删除(原则 3)。
- 去重天然:同一文件无论导入几次、改名几次,只存一份(`content_hash UNIQUE`)。
- 原始扩展名/文件名存 `source_file.original_name`,CAS 里不依赖扩展名。

```
vault/
├── medme.db                 # SQLite(L2 + FTS)
├── objects/                 # CAS(L1,不可变)
│   └── a1/b2/a1b2c3...     # 文件名=hash,不带扩展名;mime 存 DB
└── VERSION                  # vault 布局版本号
```

---

## 3. Parser

`packages/parser` 职责:把任意支持格式变成"可 OCR 的逐页位图 + 基础元数据"。

| 输入 | 处理 |
|---|---|
| PDF | 渲染逐页位图;若含文本层,直接取文本(**跳过 OCR**,更快更准) |
| JPG/PNG/TIFF | 解码;判旋转并纠正(EXIF / 版式检测) |
| DOCX | 提取文本 + 内嵌图;文本直接入 `ocr_result`(backend 记为原生) |
| CSV | 结构化,直接映射(化验批量导入,v0.2 走 clinical_event) |
| ZIP | 解包,逐个文件递归进管线 |
| DICOM | future(v0.3+) |

输出:`Vec<Page { image, dpi }>` + `DetectedMeta { mime, language, rotation }`。

> ponytail: 有文本层的 PDF/DOCX 直接取文本,别无脑 OCR。OCR 只用于"图像化"的页。

---

## 4. OCR 后端(可插拔)

见 [ADR/0002](ADR/0002-ocr-backend.md)。`packages/ocr` 定义唯一抽象:

```rust
pub trait OcrBackend {
    fn id(&self) -> &str;                 // 'onnx' | 'vlm'
    fn model_version(&self) -> &str;
    fn recognize(&self, page: &Page) -> Result<PageOcr>;
}

pub struct PageOcr {
    pub text: String,
    pub confidence: Option<f32>,
    pub layout: Option<Layout>,           // 含表格结构
}
```

- **`OnnxBackend`(默认)**:Rust 原生 ONNX Runtime + PP-OCRv5(经 `oar-ocr`),含 layout + 表格结构。进程内、无 Python、跨平台。
- **`VlmBackend`(可选)**:`PaddleOCR-VL` / `dots.ocr`,经 Ollama HTTP 或 `deepseek-ocr.rs`。仅难件(手写/复杂版式)或 v0.2 结构化抽取时显式启用。**默认不加载。**

选择策略(v0.1):固定用 onnx;VLM 由用户在设置里开启后作为"重试难件"选项。不做自动路由(YAGNI,等有数据再说)。

---

## 5. document 归纳(步骤⑤)

- `doc_type`:v0.1 用轻量规则/关键词猜测(如出现"出院记录"→ `discharge_summary`);猜不出留 `unknown`。精细分类归 v0.2 AI。
- `doc_date`:从 OCR 文本里正则/规则抽取报告日期(中英日期格式);多个候选取最靠谱一个;抽不到留 `null`(时间线归入"未定日期"分组)。
- `title`:首个标题块或原文件名。
- `page_count`:parser 给出。

> ponytail: v0.1 的 doc_type/doc_date 用规则,不上模型。规则命中率不够再换 AI——但那是 v0.2。

---

## 6. 导入方式(connectors)

| 方式 | v0.1 | 说明 |
|---|---|---|
| 拖拽 Drag & Drop | ✅ | Tauri 文件拖放事件 |
| 文件夹导入 Folder | ✅ | 递归扫描一次 |
| Watch Folder | ✅(可选) | 监听目录,新文件自动进管线 |
| 扫描 Scan | v0.2 | 系统扫描仪接口 |
| 拍照 Camera | v0.2 | 桌面摄像头/手机中转 |

所有方式最终汇入 §1 的同一条管线;差异只在"文件从哪来"。

---

## 7. 幂等与自检(必须有)

- 同一文件重复导入 → 命中 `content_hash`,返回既有 `source_file`,**不重复存储、不重复建 document**。
- OCR 可重跑:删 `ocr_result` 重跑 → FTS 重建,结果一致(纯函数于 原文件 + 模型版本)。
- 最小验证:导入 A、再导入 A 的副本(改名)→ `source_file` 仍只有 1 行,`objects/` 仍只有 1 个对象。
