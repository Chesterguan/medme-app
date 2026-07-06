# ADR 0002 · OCR 后端:Rust 原生 ONNX 默认 + VLM 可选

Status: Accepted · Date: 2026-07-06

## Context
PRD 同时列了传统 OCR(RapidOCR/PaddleOCR)与 VLM(Qwen VL/OpenMed/Ollama)。需为 local-first 的 Tauri 桌面选一个既好打包、又能覆盖难件的方案。2026 年调研发现两点变化:
1. **Rust 原生 ONNX 成熟**:`oar-ocr`(纯 Rust,ONNX Runtime,含 layout + 表格结构)、`paddle-ocr-rs`、`ocrs`——可进程内跑 PP-OCRv5,**无需 Python sidecar**。
2. **VLM 类 OCR 崛起**:`PaddleOCR-VL`(0.9B,CJK 强)、`dots.ocr`(1.7B,表格/公式 SOTA)——强但重,适合难件与结构化抽取。

## Decision
`packages/ocr` 定义单一 `OcrBackend` trait,两个实现:
- **`OnnxBackend`(默认)**:Rust 原生 ONNX + PP-OCRv5(`oar-ocr`)。进程内、跨平台、CN+EN、含表格结构。**打包干净是首要原因。**
- **`VlmBackend`(可选,默认不加载)**:`PaddleOCR-VL`/`dots.ocr`,经 Ollama 或 `deepseek-ocr.rs`。仅难件(手写/复杂版式)或 v0.2 结构化抽取时显式启用。

这是[架构里唯一刻意保留的抽象](../002_Architecture.md)——因为两个后端都是**真实需求**,非投机。

## Alternatives 拒绝理由
- **PaddleOCR + Python/PaddlePaddle**:打包与分发痛苦,违背 local-first 单文件安装。
- **Tesseract 为主**:中文与脏扫描件准确率不足。
- **纯 VLM**:逐页跑太重,CPU 用户体验差;留给难件。

## Consequences
- (+) 默认路径零 Python、易分发、速度快。
- (+) 难件/抽取有升级位,不改核心。
- (−) 维护两套后端;缓解:trait 极薄,VLM 非必装。
- 参考:[oar-ocr](https://crates.io/crates/oar-ocr) · [PaddleOCR-VL](https://huggingface.co/PaddlePaddle/PaddleOCR-VL) · [dots.ocr](https://github.com/rednote-hilab/dots.ocr) · [deepseek-ocr.rs](https://github.com/TimmyOVO/deepseek-ocr.rs)
