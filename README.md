<div align="center">

# MedMe · 医我

**个人医疗数据保险箱**

[![License: Apache 2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)
[![CI](https://github.com/Chesterguan/medme/actions/workflows/ci.yml/badge.svg)](https://github.com/Chesterguan/medme/actions/workflows/ci.yml)
[![CodeQL](https://github.com/Chesterguan/medme/actions/workflows/codeql.yml/badge.svg)](https://github.com/Chesterguan/medme/actions/workflows/codeql.yml)
[![OpenSSF Scorecard](https://api.securityscorecards.dev/projects/github.com/Chesterguan/medme/badge)](https://securityscorecards.dev/viewer/?uri=github.com/Chesterguan/medme)
[![Platform](https://img.shields.io/badge/desktop-macOS_%C2%B7_Windows-lightgrey.svg)](https://chesterguan.github.io/medme/)
[![Mobile](https://img.shields.io/badge/mobile-iOS_%C2%B7_Android-lightgrey.svg)](https://github.com/Chesterguan/medme/releases)
[![Made with Tauri + Rust](https://img.shields.io/badge/built_with-Tauri_v2_%C2%B7_Rust_%C2%B7_React-orange.svg)](#技术架构--architecture)

把你散落在各家医院的病历、化验、影像,聚合成一个
**本地优先、随身携带、完全由你掌控**的电子健康档案。

`本地优先` · `原件永存` · `端到端加密分享` · `零服务器` · `中文优先`

[官网 / Landing](https://chesterguan.github.io/medme/) ·
[医生视图样例 / Live demo](https://chesterguan.github.io/medme/demo/) ·
[加密分享查看器 / Share viewer](https://chesterguan.github.io/medme/viewer/) ·
[下载 / Releases](https://github.com/Chesterguan/medme/releases)

</div>

---

> **About (English).** MedMe (医我) is a **local-first, zero-server personal
> medical-data vault**. It collects the medical records scattered across
> hospitals, phone photos, and apps — lab reports, discharge summaries, CT/MRI
> images, DICOM — OCRs and classifies them, and aggregates everything into one
> time-ordered **health archive**. It renders each record professionally (lab
> tables, an interactive DICOM viewer) and lets you share an
> **end-to-end-encrypted, self-contained web page** with a doctor — decryptable
> in any browser with a password, no server involved. Desktop is **Tauri v2
> (Rust + React)**; an iOS capture app is in progress. Chinese-first product;
> privacy and trust are the whole point. **Not a medical device — no diagnosis
> or advice; the original documents are always authoritative.**

## 这是什么

看病多年,你的数据散落各处:一叠叠化验单、出院小结、CT 片子、手机里的报告照片、各医院 App 里互不相通的记录。换家医院、异地就医、报销、二次问诊时,你很难把它们**完整、有条理**地拿出来。

**MedMe(医我)** 把这些散落的医疗数据**收集、识别、归类、聚合**成一条清晰的「生命时间线」——按就诊/住院/转院/手术自动组织,随时可查、可搜、可导出、可安全分享给医生。就像一个只属于你的**医疗数据保险箱**:东西都在里面、只有你有钥匙、原件永远不丢。

病历产生在诊室、检查科门口、缴费窗口 —— 那时你手上只有手机,所以**手机端是主入口**(iOS + Android)。桌面端是**进阶的个人数据查看器**:大屏读片、比对多年趋势、批量整理。两端共用同一份 Rust 内核与保险箱格式。

> ⚠️ MedMe 是**数据整理工具**,不提供任何医疗建议。一切以原始医疗文件为准,并请咨询执业医师。

## 截图 / 演示 · Screenshots & demo

先到 [官网](https://chesterguan.github.io/medme/) 看产品截图与介绍;
想不安装就体验:打开 [**医生视图样例**](https://chesterguan.github.io/medme/demo/) —— 那是一份
**真实的加密分享**(与患者发给医生的完全同一条代码路径),数据换成公开演示数据集:疾病泳道
时间轴、化验趋势、12 层可滚轮翻层的 CT 阅片、PDF 原件核对。

> 📸 _演示 GIF / 视频待补充(placeholder — a demo GIF/video will go here)._

## 功能 · Features(v1.0)

- **采集 · OCR**:拖拽导入 PDF / 图片 / 文本,自动去重、原件永久保存;**自动收件箱**(手机拍照云同步到指定文件夹即自动入库);**OCR 用各平台系统自带引擎**(macOS Apple Vision · Windows Windows.Media.Ocr · iOS Apple Vision · Android ML Kit;Linux 与兜底走 PP-OCRv5,含去阴影 / 纠偏预处理)识别中文报告照片与扫描件,显示识别置信度、低置信主动警告。
- **健康档案时间线**:按就诊 / 住院 / 转院 / 手术自动聚合(时间为主键);智能日期提取、病人信息栏、中文全文搜索。
- **专业渲染 + DICOM 阅片**:按类型富渲染(化验→表格并 ↑↓ 着色、处方→用药清单、病理 / 影像 / 出院→分节);原件查看器(图片灯箱 · PDF 内嵌 · **交互式 DICOM 阅片**:窗宽窗位 / 缩放 / 多帧,支持 JPEG2000 / JPEG-LS 压缩格式)。
- **端到端加密分享**:AES-256-GCM 加密成一个自包含网页,凭口令在**任意浏览器本地解密**查看,**零服务器**。分享时可写一个建议复阅期限 —— 它是**给医生的提醒,不是访问控制**:文件在对方手里,过期后仍可打开(我们不假装能撤回)。也可一键导出自包含 HTML 交给医生打印。
- **可核验审计**:不可篡改的审计追踪(导入 / 导出 / 分享均记内容哈希 sha256);**数据安全引导**(FileVault + iCloud 高级数据保护)。

## 安装 · Install

- **推荐(无需开发环境)**:到 [Releases](https://github.com/Chesterguan/medme/releases)
  或 [官网](https://chesterguan.github.io/medme/) 下载 `.dmg`(Apple Silicon /
  Intel),拖进「应用程序」即可。
- **测试版未签名**:首次打开会被 macOS 拦住(正常)。在访达里**右键 MedMe → 打开**,
  或终端执行 `xattr -cr /Applications/MedMe.app` 后再打开。正式版将签名 + 公证。
- 详细的安装与试用步骤(含「一键加载示例数据」)见 [TESTING.md](TESTING.md)。

## 从源码构建 · Build from source

**前置依赖 / Prerequisites**

- **Rust**([rustup](https://rustup.rs);仓库固定 stable,见 `rust-toolchain.toml`)
- **Node 18+**(前端用 Vite 7,建议 **Node 20+**)、**pnpm**
- **CMake + C/C++ 工具链**——编译内置的 JPEG2000(OpenJPEG)/ JPEG-LS(CharLS)
  影像编解码。macOS:`brew install cmake`;Ubuntu:`sudo apt-get install cmake build-essential`。

```bash
git clone https://github.com/Chesterguan/medme.git
cd medme

# 桌面应用(Tauri v2 + React)
pnpm -C apps/desktop install
pnpm -C apps/desktop tauri dev      # 开发运行,热重载
pnpm -C apps/desktop tauri build    # 打包 → apps/desktop/src-tauri/target/release/bundle/

# 命令行工具(导入 / 检索,便于调试内核)
cargo run -p medme-cli -- --help

# Rust 内核:测试 / lint(提 PR 前请跑通这三条)
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

**手机端(Flutter,iOS + Android)**

```bash
cd apps/mobile_flutter
flutter pub get
flutter run                 # 接一台真机/模拟器即可,debug 构建
```

> 日常开发**不要**跑 `--release` 或多 ABI 交叉编译(很慢,且是 CI 的活)。
> 界面在 Flutter,内核仍是同一份 Rust,经 flutter_rust_bridge 调用。

> 首次编译较久(Rust 全量构建 + 编译 OpenJPEG/CharLS);首次运行 OCR 会自动下载
> ~21MB 模型到 `~/.oar`。装好的 .dmg 无需 CMake(编解码已静态打包)。

## 技术架构 · Architecture

- **桌面**:Tauri v2(Rust 后端 + React / TypeScript 前端,Tailwind v4)。
- **存储(唯一真相)**:事件溯源——追加式事件日志(`log/*.jsonl`)+ 内容寻址存储
  (CAS,sha256)。SQLite(FTS5 中文分词)是**可随时重建的派生缓存**:删库可从日志
  零损失重建。日志按设备分段,天然免冲突。
- **Rust 工作区(crate)**:

  | crate | 职责 |
  |---|---|
  | `core-model` | 事件日志、CAS、物化、审计、查询 |
  | `parser` | 文档分类、日期提取、结构化 |
  | `pipeline` | 导入编排(去重、OCR 调度、入库) |
  | `ocr` | 按平台 OCR:macOS Apple Vision · Windows Windows.Media.Ocr · Linux/兜底 PP-OCRv5(去阴影 / 纠偏预处理)。移动端图片 OCR 另在 Flutter/Dart 层:iOS Apple Vision · Android ML Kit |
  | `dicom` | DICOM 解析与渲染(JPEG2000 / JPEG-LS 解码) |
  | `medme-share` | 端到端加密分享导出 |

- **应用**:`medme-cli`(命令行)· `medme-desktop`(Tauri v2 桌面)·
  `apps/mobile_flutter`(Flutter,iOS + Android)。移动端界面用 Flutter,**内核仍是同一份
  Rust**(结构化、加密、事件溯源、保险箱格式);识别层例外,走各平台系统自带 OCR。
- **多端同步**:把数据保险箱放进你自己的云同步文件夹;追加式日志天然免冲突,多设备
  各自重建即可,无需服务器。

## 路线图 · Roadmap

- ✅ **v1.0(已完成)**:影像 / DICOM 管线与阅片、端到端加密分享 + 在线预览器、
  事件溯源存储、示例数据。
- 🚧 **手机端(进行中)**:Flutter 应用,iOS + Android 双端,与桌面共享同一份 Rust 内核
  与保险箱格式。安卓构建已发布到 [Releases](https://github.com/Chesterguan/medme/releases);
  iOS 走 TestFlight(见 [`docs/013_Mobile_App.md`](docs/013_Mobile_App.md))。
- 🔭 **计划中**:AI 健康洞察(在本地、可溯源回原件的前提下辅助理解)· 导出按时间范围
  选择 · 可选口令加密(适配 iCloud 之外的第三方云)· FHIR / OMOP 导出。

## 隐私与安全 · Privacy & security

- **零服务器**:没有后端,我们看不到、也不持有你的任何数据。
- **端到端分享**:分享文件在你和医生的设备本地加解密,云盘 / 传输途中都是密文。
- **可核验**:所有导入 / 导出 / 分享由追加式事件日志 + 内容哈希记录,防篡改、可审计。
- 发现安全漏洞?请**私下**上报,见 [SECURITY.md](SECURITY.md)——不要开公开 issue。

## 参与贡献 · Contributing

欢迎 issue 与 PR!先读 [贡献指南 CONTRIBUTING.md](CONTRIBUTING.md) 与
[行为准则 CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md);人工验收要点见 [TESTING.md](TESTING.md)。

**想快速上手的话:**

- **先跑起来**:`cargo test --workspace` 能过,说明环境没问题;再 `cargo run -p medme-cli -- --help`
  拿命令行导入几份 `examples/sample-records/` 里的示例病历,五分钟摸清内核在干什么。
- **理解设计决策**:架构决策记录在 [`docs/ADR/`](docs/ADR/)(Nygard 格式,不可变,新决策
  supersede 旧号);工程日志在 [`docs/log/`](docs/log/) —— 讨论过程、实测数字、踩过的坑都在里面。
- **提 PR 前**:`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、
  `cargo test --workspace` 三条都要过(CI 也这么卡)。
- **医疗数据的红线**:结构化抽取一律**确定性、可解释**,数值只能取自原件,不接受"模型觉得是"。
  改到抽取或分享路径时,请附上你实际跑过的验证。

**安全问题请勿开公开 issue** —— 见 [SECURITY.md](SECURITY.md) 私下上报。

## 声明 · Disclaimer

MedMe 为个人医疗数据整理与保存工具,**非医疗器械,不提供任何医疗诊断或建议**,一切
以原始医疗文件为准并请咨询执业医师。数据的调取须经本人或其法定责任人同意。软件按
「现状」提供;因采用去中心化存储,若数据遗失我们不一定能协助找回。完整声明见应用内
「关于 · 声明」。

## 许可证 · License

[Apache-2.0](LICENSE) © 2026 MedMe Team.

---

<div align="center">
<sub>© MedMe Team 2026 · 本地优先的个人医疗数据保险箱</sub>
</div>
