# ADR 0006 · iOS 移动端图片 OCR 改用 PP-OCRv5(取代 [0005](0005-ocr-per-platform-native.md) 的 iOS=Apple Vision 一行)

Status: Accepted · Date: 2026-07-22 · **Amends/Supersedes [0005](0005-ocr-per-platform-native.md)(仅 iOS 移动端那一行)**

## Context
[0005](0005-ocr-per-platform-native.md) 记录移动 iOS 图片 OCR 用 **Apple Vision**。2026-07-22 真机(iPhone 14 Pro)实测发现:Apple Vision 在真实中文化验单照片上出**临床危险的认数字错**——红细胞计数 4.35→1.35、红细胞平均体积 90.1→50.1、红细胞压积 39.2→19.2、中性粒细胞% 43.60→4.60(一位数字错 = 正常读成异常);同一批图 PP-OCRv5 全对。团队会议定的 Point 1 是「识别质量是重中之重」(中国用户在意便利/有用,不是隐私)。用户真机验收 PP 版(build 29)确认「认得准、渲染也不错、符合要求」。

## Decision
移动 iOS 图片 OCR 从 Apple Vision 改为 **PP-OCRv5**(`packages/ocr` 的 `engine` 路径:oar-ocr + ONNX Runtime),经 FRB `recognize_image_pp` 调用(`apps/mobile_flutter/lib/ocr_bridge.dart:30` iOS 分支)。不再走 `AppDelegate.swift` 的 Vision(代码暂留但未调用)。

- **模型随 app 打包**:PP-OCRv5 det + rec + 字典(~20MB)用 `include_bytes!` 编进 Rust 静态库,首用落沙盒、`ocr::set_model_dir` 指向(iOS 沙盒无可写 `$OAR_HOME`,关 `auto-download`)。
- **列对齐**:PP 走 Rust,故坐标→列对齐重建移植进 `packages/ocr`(`rebuild_layout_text`,additive),输出查看器 `splitCells` 要的空格对齐格式。
- **溯源**:落库 backend 标 `OcrBackendKind::Onnx`(`vault.rs` 按 `cfg(target_os="ios")` 区分,安卓仍 `MlKit`)。
- **链接约束(踩坑记录)**:pyke 预编译 `libonnxruntime.a` 内部含自相冲突的重复对象,与 cargokit 默认 `-force_load` 整库不兼容(757 duplicate symbol)。改为**弃 force_load、按需链接** + 用 `-Wl,-u` 把 14 个 FRB 运行时符号钉成链接根 + 补 `-lc++ -framework CoreML`(`rust_builder/ios/rust_lib_mobile_flutter.podspec`)。详见 [log 2026-07-22](../log/2026-07-22-ios-pp-ocr-scanner-linker.md)。

## Consequences
- (+) 修掉 Apple Vision 的危险认数字错——化验单数值是医疗安全底线。
- (+) iOS 与桌面(mac Vision 主 + PP 兜底、Win 同理、CLI/Linux PP)在 PP 这条路上更统一。
- (−) IPA 实测 **+~29MB**(59→88MB;onnxruntime 静态库经 strip/死代码消除贡献 ~11MB + 模型 ~18MB)。CPU 推理比 Vision 的 ANE 慢 2–3×。
- 采集入口配套:文档扫描器(VisionKit `VNDocumentCameraViewController`)复活做拍照纠偏,把斜拍拉正喂给 PP(斜拍/旋转件 PP 单独识别会乱)。
- **未变**:0005 的桌面 macOS(Apple Vision 主 + PP 兜底)、Windows(Windows.Media.Ocr 主 + PP 兜底)、CLI/Linux(PP)、移动 Android(ML Kit)各行不变。Android 是否也换 PP 单独评估(用户倾向换,待建 + 验证 oar-ocr 在 Android 交叉编译)。
