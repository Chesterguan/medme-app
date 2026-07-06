# 006 · Plugin SDK · 插件与连接器

> 面向 v0.3+。现在只定契约,防止把外部集成硬编码进核心。**不在 v0.1/v0.2 实现。**

关联:[002_Architecture](002_Architecture.md) · [004_Import_Pipeline](004_Import_Pipeline.md) · [005_AI_Principles](005_AI_Principles.md)

---

## 1. 为什么现在只定契约

PRD 的 roadmap 里,插件/连接器要接:医院 API、Apple Health、Health Connect、可穿戴、科研平台、云同步。这些**不该进核心**——核心只认 `packages/*` 里已定义的三类扩展点。现在把扩展点定清楚,以后集成都往里插,核心不动。

> ponytail: 不为一个尚不存在的插件生态写运行时/沙箱/市场。v0.3 有第一个真实插件时,再按那时的需要落地加载机制。现在就是三个 trait。

---

## 2. 三类扩展点(复用已有抽象)

| 扩展点 | trait | 已在 | 例子 |
|---|---|---|---|
| **导入源 Connector** | `ImportSource` | [004](004_Import_Pipeline.md) connectors | 文件夹、Watch、扫描仪、Apple Health 导出、医院 API |
| **OCR 后端** | `OcrBackend` | [004](004_Import_Pipeline.md) §4 | onnx、VLM、第三方 OCR |
| **AI Provider** | `AiProvider` | [005](005_AI_Principles.md) §3 | 本地 Ollama、云 API |

再加一个导出扩展点:

| **导出目标 Exporter** | `Exporter` | 本文 §3 | FHIR、JSON、PDF、ZIP、第三方 |

---

## 3. Connector / Exporter 契约

```rust
pub trait ImportSource {
    fn id(&self) -> &str;
    // 产出待导入文件流;每个 item 交给 §004 的统一管线
    fn pull(&self, ctx: &ImportCtx) -> Result<Vec<RawInput>>;
}

pub trait Exporter {
    fn id(&self) -> &str;                 // 'fhir' | 'json' | 'pdf' | 'zip'
    fn export(&self, query: &ExportQuery, out: &mut dyn Write) -> Result<()>;
}
```

- 所有 connector 产出的文件都走 [004](004_Import_Pipeline.md) 的同一条 CAS + OCR 管线——**没有绕过 CAS 的后门**(原始永存)。
- 所有 exporter 只**读** L2/L1,永不改数据(用户带走数据的能力,原则:随时可离开)。
- FHIR / OMOP 只在 exporter/mapper 里出现,不进核心存储([ADR/0004](ADR/0004-core-model-independence.md))。

---

## 4. 加载机制(v0.3 再定)

首版可以是**编译期注册**(内置插件列表),不做动态加载/沙箱。真正的第三方动态插件、权限模型、签名校验等,等有外部开发者需求时再设计。此处**故意留白**。
