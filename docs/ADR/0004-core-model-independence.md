# ADR 0004 · Core Model 独立于 FHIR / OMOP

Status: Accepted · Date: 2026-07-06

## Context
原则 5 要用开放标准(FHIR 互操作、OMOP 术语),但若把 FHIR 资源结构或 OMOP 宽表直接当内部存储,会把它们的复杂度与版本耦合进核心,违背原则 6(长期稳定)与 local-first 的轻量诉求。

## Decision
**Clinical Core Model([003](../003_Core_Data_Model.md))是唯一真相层,独立于 FHIR 与 OMOP 表结构。**
- L2 只存 `code_system + code + display`;术语映射(→ OMOP concept 等)在 L3 单独做。
- FHIR / OMOP 只作为**导出/映射目标**出现在 exporter/mapper([006](../006_Plugin_SDK.md)),不进核心存储。
- 核心 schema 为本地应用(时间线/搜索/查看)优化,简单、稳定、可长期打开。

## Consequences
- (+) 核心不被 FHIR 版本演进牵动;避免供应商/标准锁定。
- (+) 导出仍完整支持 FHIR/JSON(用户可离开)。
- (−) 需维护 L2→FHIR/OMOP 映射层;这是刻意的边界,而非重复。
