# 005 · AI Principles · AI 原则

> 面向 v0.2+。v0.1 只有"整页文本"这一层最基础的 AI(OCR)。本文先定原则与接口,防止后续走偏。

关联:[000_Vision](000_Vision.md)(原则 4)· [003_Core_Data_Model](003_Core_Data_Model.md) · [006_Plugin_SDK](006_Plugin_SDK.md)

---

## 1. 铁律

1. **AI 是辅助,不是真相。** AI 抽取、总结、整理;**永不修改或替代原始文件**。
2. **一切可溯源。** 每条 AI 结果必须能指回来源:`clinical_event.source_ocr_id` + `source_span`(字符偏移),再经 `ocr_result` 指回 `source_file`。UI 上任何结论都能"跳回原文高亮"。
3. **一切可重算。** 记录 `backend` + `model_version`。换更好的模型 → 重跑原文件 → 覆盖派生层,原始层不动。
4. **置信度显式。** 每条抽取带 `confidence`;低置信结果在 UI 标注,不静默当真。
5. **默认本地。** 无遥测;云/在线模型必须用户显式开启,并提示数据出境。

---

## 2. AI 管线(v0.2+)

```
原文件 → OCR(text+layout)→ Medical Parser → Entity Extraction → clinical_event → Timeline
```

- **Medical Parser**:把 OCR 文本按文档类型切成结构区块(化验表、用药清单、诊断段…)。
- **Entity Extraction**:从区块抽 `clinical_event`(诊断/化验/用药/操作/影像/过敏/生命体征),带 `code_system+code`(L3 映射)与 `source_span`。
- 表格类(化验单)优先用 layout 里的 `Table` 结构 → 数值/单位/参考区间对齐,而非纯文本猜。

---

## 3. Provider 抽象

AI 能力挂在可插拔 provider 后面(与 OCR 后端同思路):

```rust
pub trait AiProvider {
    fn id(&self) -> &str;
    fn extract(&self, doc: &DocumentText) -> Result<Vec<ClinicalEventDraft>>;
    fn summarize(&self, doc: &DocumentText) -> Result<String>;
}
```

- 本地:Ollama 兼容(Qwen VL / OpenMed 等)、`deepseek-ocr.rs`。
- 云:显式配置的 API provider(v0.3 插件系统提供)。
- 输出永远是 **Draft**:写入前可人工确认(尤其低置信),确认与否都保留溯源。

---

## 4. Consensus(future)

多模型共识抽取:同一文档跑多个模型,结果做一致性投票,自动估计置信度。
- 冲突项标注、供人工裁决。
- 属长期方向,**不在 v0.2 首版范围**(YAGNI,先有单模型抽取跑通)。

> ponytail: 先单模型 Draft + 人工确认。consensus 是等单模型不够用、且有多个可用模型时才做的优化。
