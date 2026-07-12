# 移动端重做:Flutter UI + Rust 核(flutter_rust_bridge)

> 2026-07-12 决定。用户批准方案 A。取代 Tauri v2 移动端(WebView 界面层造成的 PDF 白屏 /
> 文字截断 / 打包折腾等弯路)。**桌面端不变,仍用 Tauri。**

## 目标
用 **Flutter 原生 UI** 重做 iOS/安卓 app,功能与设计风格保持不变;**复用现有 Rust 数据核**
(保险箱 CAS+追加日志、DICOM 编解码、加密分享)经 `flutter_rust_bridge`(FRB v2)调用,
保证与桌面端**同一保险箱格式 → 跨设备同步天然互通**。

## 架构

```
Flutter (Dart) UI  ──FRB──▶  crates/mobile_ffi (Rust)  ──▶  core-model / pipeline / share / dicom / parser
   原生界面/导航/PDF查看/相机                薄封装:把 vault 操作暴露成 async Dart API
   ML Kit 文字识别(离线中文)
```

- **UI = Flutter**:所有屏幕、导航、PDF 查看(`pdfx`/`syncfusion` 等成熟插件)、图片查看、相机/相册/文件选择,全部原生组件。再无 WebView。
- **数据核 = 现有 Rust crate**,经新增的 `crates/mobile_ffi` 薄封装暴露。**不重写保险箱**(否则同步断、且推倒最难最已测透的代码)。
- **OCR = Flutter 插件 `google_mlkit_text_recognition`**(iOS+安卓离线中文)。Flutter 拍照/选图 → ML Kit 识别 → 把「原始字节 + 识别文本 + 置信度」交给 Rust `ingest_image_with_text` 落库。**不再维护 Rust 的 Vision/MLKit FFI 桥**。PDF 文本层抽取、DICOM 元数据仍在 Rust。

## 复用 vs 新建
- **复用(不动)**:`packages/core-model`(Vault/CAS/log/HMAC)、`packages/pipeline`(ingest 编排)、`packages/share`(加密分享+导出)、`packages/dicom`、`packages/parser`。桌面端继续用。
- **新建**:
  - `crates/mobile_ffi`:FRB 封装层。依赖上述 crate,暴露干净 async API。
  - `apps/mobile_flutter/`:Flutter 工程(FRB 生成的 Dart 绑定 + UI)。
- **删除(达到功能对齐后)**:`apps/mobile`(Tauri v2 移动端)。

## FFI API 面(mobile_ffi 暴露给 Flutter)
镜像现有 `apps/mobile/src-tauri/src/commands.rs` 的能力:
- `open_vault(docs_dir, data_dir, icloud_enabled) -> ()`(决定真相根:iCloud 容器 or 沙盒)
- `load_archive() -> Vec<TimelineGroup>`
- `get_document(id) -> DocumentDetail`
- `read_source_bytes(id) -> Vec<u8>` / `render_dicom_png(id) -> Vec<u8>`
- `ingest_file(path)` / `ingest_bytes(name, bytes)`(PDF/TXT/DICOM 走 pipeline)
- `ingest_image_with_text(name, bytes, ocr_text, confidence)`(图片:Flutter 已用 ML Kit 识别)
- `create_share(expires_days) -> ShareResult`(自包含加密 HTML)
- `export_timeline_html(from_date?, to_date?) -> ExportResult`(**带日期区间筛选**)
- `patient_profile() -> PatientProfile`
- `reset_vault()`、`load_demo_data()`
- `icloud_status()` / `enable_icloud_sync()` / `disable_icloud_sync()`(iOS)

DTO 用 FRB 的镜像结构(Rust struct → Dart class 自动生成)。

## 屏幕清单(还原现有设计:teal、卡片、底部导航)
底部导航按用户新要求调整:
1. **导入导出**(用户明确要求提升为一级 tab):相机/相册/文件导入 + 导出(时间线 HTML,带日期区间筛选,后续可加更多筛选维度)。
2. **健康档案**(时间线:就诊组 + 独立文档,点开详情)。
3. **设置**(载入示例数据 / 清空重置 / iCloud 同步 / 加密分享入口 / 关于)。
- 文档详情:内容感知渲染(化验表格/处方卡/病历)+ 原件查看(图片查看器 / PDF 插件 / DICOM 只显元信息文本)。

## 同步(iCloud,iOS)
真相(objects/+log/)放 iCloud ubiquity 容器,派生 db 留沙盒 —— 沿用现 Rust `icloud` 逻辑,
路径由 Flutter/iOS 配置容器 entitlement,`open_vault` 里决定根。安卓跨设备 → 后续(1.3 云盘/QR)。

## 构建/发布
- iOS:`flutter build ipa`(Flutter 标准流程,比 Tauri 顺;签名/描述文件复用现有 `MedMe App Store` + ASC key)→ TestFlight。
- 安卓:`flutter build apk`。
- Rust 库经 FRB 的 `cargokit` 在 flutter build 时自动编 iOS/安卓静态库并链接。

## 分阶段
- **P1 骨架**:`crates/mobile_ffi` + `flutter create` + FRB init;跑通一个最小 Rust 调用(open_vault + record_count)在 iOS 模拟器显示。**验证工具链闭环**。
- **P2 FFI 全量**:暴露上面全部 API + DTO。
- **P3 UI**:三个 tab + 文档详情,还原设计。
- **P4 OCR/PDF/图片**:ML Kit 识别 + PDF 插件 + HEIC/图片。
- **P5 iCloud 同步**。
- **P6 导出日期筛选 + 打磨**。
- **P7 出包**:iOS TestFlight + 安卓 APK;达标后删 `apps/mobile`(Tauri)。

## 风险
- FRB iOS/安卓构建集成(cargokit)是最需要跑通的一环 → P1 先验证。
- ML Kit 中文识别质量需真机验(和之前一样,发前门槛)。
- 保留桌面同款保险箱格式是硬约束(用 FRB 复用 Rust 核天然满足)。
