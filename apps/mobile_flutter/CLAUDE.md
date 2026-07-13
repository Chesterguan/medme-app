# MedMe 移动端(Flutter)—— 给 Claude Code 的构建纪律

架构:Flutter UI + 复用现有 Rust core(经 flutter_rust_bridge / cargokit FFI)。
保险箱格式与桌面 Tauri 版逐字节一致,保证同步兼容。

## Build rules(硬规则,不得违反)

- **Never** run full Flutter release builds during routine development.
- **Never** build all Rust mobile targets / all ABIs unless explicitly requested.
- Default to **debug** builds for the **currently selected** simulator/device only
  (`flutter run` or `flutter run -d <device-id>`)。不要每次同时编译全部 ABI,不要日常用 `--release`。
- Do **not** run `flutter build ipa`、`flutter build appbundle`、`flutter build apk --release`,
  或任何多 ABI / release 的 Rust 交叉编译,**除非用户明确许可**。
- 日常验证只用:`flutter analyze`、`dart test`、Rust 单元测试。这些才是改 UI 后的验证手段。
- **完整 Rust release 交叉编译是发布流程(CI 的活),不是 Claude Code 的日常验证步骤。**
- Rust release artifacts / 安装包 / iOS archive / TestFlight / 全 ABI Android release
  **一律由 GitHub Actions 产出**(见 `.github/workflows/mobile.yml`),带 cargo/target/pub/gradle 缓存。
- All long-running build commands must use a timeout。
- **Before invoking any command expected to take more than 5 minutes, STOP and report
  the exact command to the user instead of running it.**(别擅自触发 30 分钟后台构建。)

## 为什么(踩过的坑)

本地反复跑 `flutter build ipa --release` / release APK 当"验证",每次 30+ 分钟
(cargokit 每次现场重编 Rust release,不复用缓存),还被环境 kill / flutter temp 崩溃,
纯浪费时间。release 跨平台编译属于发布流程,搬去 CI 用 `Swatinem/rust-cache` 缓存即可。

## 版本纪律

用**最成熟稳定**版本,别用 bleeding-edge:
- 安卓:AGP 8.9.1 / Gradle 8.11.1 / Kotlin 2.1.0(FRB 模板默认给的 AGP 9 / Gradle 9 不兼容 Flutter Gradle 插件)。
- 插件对齐 win32 版本:file_picker ^8.x 与 share_plus ^11.x(share_plus 13.x 要 win32^6 会把 file_picker 顶回无 namespace 的老版)。
- iOS 部署目标 15.5(ML Kit 要求);模拟器 `EXCLUDED_ARCHS=arm64`(ML Kit 无模拟器切片)。

## 出包(仅发布时,优先 CI)

- iOS bundle id = `com.medme.mobile`(复用现有 App Store app + 「MedMe App Store」描述文件)。
- 安卓 `applicationId`/`namespace` 已对齐 `com.medme.mobile`(kotlin 目录已改名);iOS/安卓统一 `com.medme.mobile`。
