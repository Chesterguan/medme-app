# ADR 0001 · 中英文一等公民

Status: Accepted · Date: 2026-07-06

## Context
目标用户既有中国大陆医院报告(中文),也有欧美体系报告(英文),且需对接 MIMIC-OMOP(英文)测试库。语言选型影响 OCR 模型、术语库、搜索分词。

## Decision
**中文与英文均为一等公民。**
- Clinical Core Model **语言无关**:存原文 `display` + 归一化 `code_system/code`([003](../003_Core_Data_Model.md))。
- OCR 用支持 CJK+拉丁的 PP-OCRv5([ADR/0002](0002-ocr-backend.md))。
- 搜索:中文预分词(`jieba-rs`)+ FTS5 `unicode61`,英文按空格([003](../003_Core_Data_Model.md) §5)。
- `document.language` 记 `zh/en/mixed`,不影响存储结构。

## Consequences
- (+) 单一模型/管线覆盖两种语言,无分叉。
- (+) 可直接用 MIMIC-OMOP 做测试。
- (−) 中文分词是额外一步;精度不够时升级 `trigram`。
