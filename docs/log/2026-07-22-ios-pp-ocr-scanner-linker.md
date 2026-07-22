# 2026-07-22 · PP-OCRv5 上 iOS 真机 + 文档扫描器复活 + 链接硬骨头

> 工程日志。精炼 + 链接,不重讲 git/issue。承接 [2026-07-18 夜](2026-07-18-qr-share-security-and-community-prep.md)。

目标:把「拍照识别质量」做到位(团队会议定的 Point 1——中国用户在意便利/有用,不是隐私)。真机(iPhone 14 Pro)验收通过:**文档扫描器纠偏 + PP-OCRv5 认字 + 坐标列对齐渲染,三合一跑通**,用户确认「认得准、渲染也不错、符合要求」。

分支 `feat/ios-pp-ocr-test`(build 29),**尚未合 main**——采纳与否见文末待决。

## PP-OCRv5 vs Apple Vision:数字认错是安全问题

桌面用真实中文化验单照片实测,Apple Vision 出**临床危险的认数字错**,PP 全对:

| 项目 | 原件 | Vision | PP |
|---|---|---|---|
| 红细胞计数 | 4.35 | **1.35** | 4.35 |
| 红细胞平均体积 | 90.1 | **50.1** | 90.1 |
| 红细胞压积 | 39.2 | **19.2** | 39.2 |
| 中性粒细胞% | 43.60 | **4.60** | 43.60 |

一位数字错 = 正常读成异常。这是换 PP 的真价值。真机上 PP 走 `packages/ocr` 的 `engine`(oar-ocr + ONNX Runtime),`ocr_bridge.dart:30` iOS 分支调 FRB `recognize_image_pp`,绕过 Vision。**caveat**:PP 只修认字不修排版(排版靠下面的列对齐);斜拍/旋转扫描件 PP 也会乱(靠扫描器纠偏)。代价 ~90MB 包体(onnxruntime 静态库 73MB + PP 模型 20MB)、CPU 推理比 Vision 的 ANE 慢 2–3×。

## 渲染:坐标 → 列对齐(引擎无关,两个引擎都要)

查看器/手机端把化验单渲染成表格靠文本里**列对齐的空格**(`splitCells` 按「≥2 连续空格」切列)。OCR 输出没有这种空格 → 流水账。修法:OCR 保留每行 boundingBox,按 y 归「视觉行」、行内按 x(相对整页文字边界归一化,非整图宽度)映射字符列补空格。iOS(Vision)/Android(ML Kit)在原生/桥接层各实现一份;PP 走 Rust,故列对齐逻辑也移植进 `packages/ocr`(`rebuild_layout_text`,additive,桌面 CLI 路径不动)。散文单片段行原样输出不受影响。

## 文档扫描器「点拍照没反应」的真因:全新安装才弹权限

v23–v27 追了六版 permission_handler 的 SPM/宏/env——**全是治错病**。真因:每次 TestFlight 是**升级**不是全新安装,iOS 相机权限提示从不触发 = 权限从未授予。用户删除重装后普通相机、文档扫描器都正常弹权限、都能用。**教训**:iOS 权限行为在升级 vs 全新安装下不同,真机验收权限类问题必须干净重装。v27 撤掉扫描器其实非必要;本轮 un-revert 复活(`cunning_document_scanner`,VisionKit `VNDocumentCameraViewController`)。

第二个真 bug:我们给交互式扫描器套了 `.timeout(12s)` 兜底 → 用户扫描超时后 Dart 放弃、在仍开着的扫描器上叠个普通相机 → 点保存「不往后执行」。交互式 UI 不该有 wall-clock 超时,取消由插件返回空处理。已去掉。VisionKit 是多页扫描器(`noOfPages` 在 iOS 不生效),靠右上角「保存」结束。

## iOS 接 PP 的链接硬骨头(全诊断,非猜)

pyke 预编译的 `libonnxruntime.a`(73MB)**内部含 28 个自相冲突的重复对象**(如 `onnx-ml.pb.cc.o` 两份、彼此各有独有符号)。普通按需链接无碍(链接器每符号取首个定义、忽略后续,spike 独立 bin 就这么链过);但 cargokit 默认 `-force_load` 整个 Rust 静态库(为防 Dart 运行时 dlsym 的 FRB 入口被死代码消除),force_load 会拉入每个对象 → **757 duplicate symbol**;删任一份 → 又缺符号。此库无法整库 force_load。

**修法**(本地 aarch64-apple-ios 实链验证:0 dup、0 undef,14 符号全进 dyld 导出表):
1. 弃 `-force_load`,改让 onnxruntime 按需链接。用 `-Wl,-u,<sym>` 把 FRB 运行时那 14 个固定符号(`frb_pde_ffi_dispatcher_*` 等)钉成链接根——单一 dispatcher 静态引用所有 wire 函数,保留它就按需拉入整条闭包(wire → ocr → ort → onnxruntime)。这批 `_frb_*` 是运行时固定符号,bridge 重生成不变。
2. 补 `-lc++ -framework CoreML`:ort build script 发的 link-lib 只在 cargo 自己链接时生效,Xcode 做最终链接时传不过来。

都落在 `apps/mobile_flutter/rust_builder/ios/rust_lib_mobile_flutter.podspec`。

## 发布流程两个坑(顺手修了)

- **build number 撞号**:`flutter build ipa` 用 pubspec 的 `+27`,render-fix / PP 各版都沿用 27,和相机版撞号被 App Store Connect 拒(bundle version 必须递增)——所以真机上一度只有 Apple Vision 那版。每版手动 bump(28、29)。
- **altool 假成功**:上传失败(如撞号)会打 `ERROR` 却 `exit 0` → CI 步骤假绿、掩盖撞号。`mobile.yml` 上传步骤加 `pipefail` + 扫 `ERROR` 命中即 `exit 1`。

## 待决(下次)

- **是否采纳 PP 上 iOS = 合 main + supersede [ADR 0005](../ADR/0005-ocr-per-platform-native.md)**(它现在写 iOS=Apple Vision)。成本:~90MB 包体、CPU 慢 2–3×。用户已真机确认质量达标,待拍板。
- 采纳后:Vision 代码留作 fallback 还是删?Android 是否也从 ML Kit 换 PP(用户反馈安卓识别也不好)?桌面已有 PP 兜底路径。
- `ingest_image_with_text`/`backfill_pdf_text` 在移动端硬编 `OcrBackendKind::MlKit`,iOS 用 PP 时溯源元数据标错引擎(测试版无碍,采纳前要修)。
