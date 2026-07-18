# ADR 0005 · OCR 后端:各平台原生优先 + PP-OCRv5 兜底 + 移动端 Dart 原生

Status: Accepted · Date: 2026-07-18 · **Supersedes [0002](0002-ocr-backend.md)**

## Context
[0002](0002-ocr-backend.md) 定的是桌面「Rust 原生 ONNX(oar-ocr/PP-OCRv5)默认 + VLM 可选」。代码演进后现实已不同(2026-07 逐模块审计读代码确认):桌面 mac/Win 实际用系统原生 OCR 为主,移动端是 Flutter、图片 OCR 在 Dart 原生层,PP-OCRv5 只在部分端。0002 因此**过时**,本 ADR 取代它、记录当前真相。

## Decision(现状,有代码依据)
按平台路由,不是单一后端:

| 端 | 图片 OCR | 出处 |
|---|---|---|
| 桌面 macOS | **Apple Vision** 主,PP-OCRv5(oar-ocr)兜底 | `packages/ocr/src/lib.rs`(`recognize`,`#[cfg(macos)]`) |
| 桌面 Windows | **Windows.Media.Ocr** 主,PP-OCRv5 兜底 | `packages/ocr/src/lib.rs`(`#[cfg(windows)]`) |
| CLI / Linux | **PP-OCRv5**(oar-ocr,ONNX,`feature=engine`) | 同上(else 分支) |
| 移动 iOS | **Apple Vision**(Swift,`medme/ocr` MethodChannel) | `apps/mobile_flutter/ios/Runner/AppDelegate.swift` + `lib/ocr_bridge.dart` |
| 移动 Android | **Google ML Kit 中文** | `apps/mobile_flutter/lib/ocr_bridge.dart` |

- **移动端不走 Rust `ocr` crate**:图片 OCR 在 Dart 层(Vision/ML Kit),`pipeline` 依赖的 `ocr` crate 在移动 build 关掉 `engine`(`packages/pipeline/Cargo.toml`),故移动端**扫描 PDF** 由 Flutter 侧 `pdfx` 渲染→图片 OCR 回填(`backfill_pdf_text`)。
- **Layer-0**:OCR 保留块结构(ML Kit blocks / Vision 排序+分块),供下游按段分块(见 `docs/log/2026-07-18-*`)。
- **VLM(MedGemma)** 只做过抽取探索,**未集成进产品**;方向见 issue #150、小模型 LoRA 见 #157。

## Consequences
- (+) 各端用系统最强原生引擎(中文更好、HEIC 原生),打包干净。
- (−) 移动与桌面 OCR 路径不同,得分别验证;PP-OCRv5 ≠ 移动端引擎(易记混,0002 就把这搞混过)。
- 抽取层(正则 → MedGemma)是独立演进线,见 [030](../030_Clinical_Handoff.md) 与 issue #141/#148/#150。
