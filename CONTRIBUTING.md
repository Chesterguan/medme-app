# 贡献指南 / Contributing to MedMe

感谢你有兴趣改进 MedMe(医我)!本文说明如何搭环境、跑测试、以及提 PR 的规范。
中英文皆可,提交信息与讨论用中文或英文都行。

Thanks for your interest in improving MedMe. This guide covers setting up a dev
environment, running tests, and our pull-request conventions. Chinese or English
are both welcome.

> 请先阅读并遵守 [行为准则 (Code of Conduct)](CODE_OF_CONDUCT.md)。
> 安全漏洞请**私下**上报,见 [SECURITY.md](SECURITY.md) —— 不要开公开 issue。

---

## 开发环境 / Development setup

### 前置依赖 / Prerequisites

- **Rust**(通过 [rustup](https://rustup.rs);仓库固定 stable 工具链,见
  `rust-toolchain.toml`)
- **Node 18+**(前端构建用 Vite 7,建议 **Node 20+**,Vite 7 要求
  `^20.19 || >=22.12`)
- **pnpm**(`npm i -g pnpm`)
- **CMake + C/C++ 工具链** —— 用于编译内置的 JPEG2000(OpenJPEG)/ JPEG-LS
  (CharLS)影像编解码。macOS:`brew install cmake`;Ubuntu:
  `sudo apt-get install cmake build-essential`。

> 首次构建会全量编译 Rust,并编译 OpenJPEG/CharLS,较慢。首次运行 OCR 会自动下载
> ~21MB 的 PP-OCRv5 模型到 `~/.oar`。

### 克隆与运行 / Clone and run

```bash
git clone https://github.com/Chesterguan/medme-app.git
cd medme-app

# 桌面应用(Tauri v2 + React)
pnpm -C apps/desktop install
pnpm -C apps/desktop tauri dev      # 开发运行,热重载
# 打包:pnpm -C apps/desktop tauri build
#   → 产物在 apps/desktop/src-tauri/target/release/bundle/

# 命令行工具(用于导入/检索,便于调试内核)
cargo run -p medme-cli -- --help
```

---

## 项目结构 / Project layout

事件溯源是唯一真相:**追加式事件日志(`log/*.jsonl`)+ 内容寻址存储(CAS,
sha256)** 为源头,SQLite(FTS5 中文分词)是可随时重建的派生缓存。

```
packages/
  core-model/   存储与检索:事件日志、CAS、物化(materialize)、审计、查询
  parser/       文档分类、日期提取、结构化
  pipeline/     导入编排(去重、OCR 调度、入库)
  ocr/          PP-OCRv5 封装(含去阴影/纠偏预处理)
  dicom/        DICOM 解析与渲染(含 JPEG2000 / JPEG-LS 解码)
  share/        端到端加密分享导出(crate 名 medme-share)
apps/
  cli/          medme-cli:命令行(导入/检索/调试)
  desktop/      Tauri v2 桌面壳(src-tauri 为 Rust 端 medme-desktop)+ React 前端
  mobile/       iOS 端(由手机端负责人维护,请勿在无关 PR 中改动)
docs/           设计文档、架构、ADR
examples/       示例数据集(demo-dataset)、脱敏样本、红队素材
web/            hosted-viewer(在浏览器里解密查看加密分享)
```

Rust 工作区 crate 名(供 `cargo -p` 使用):`core-model`、`parser`、`pipeline`、
`ocr`、`dicom`、`medme-share`、`medme-cli`、`medme-desktop`、`medme-mobile`。

---

## 编码规范 / Coding standards

### Rust

- **格式化**:提交前运行 `cargo fmt --all`,不要留下格式差异。
- **Lint**:`cargo clippy --all-targets -- -D warnings` 必须无告警(至少对你改动的
  crate)。CI 会对核心库 crate 执行
  `cargo clippy -p core-model -p parser -p pipeline -p ocr -p dicom -p medme-share --all-targets -- -D warnings`。
- **生产代码禁止 `unwrap()`**:用有类型的错误(`thiserror`/`anyhow`)或带**注释说明
  不变量**的 `expect("…")`。测试代码里可以用 `unwrap()`。
- **禁止无谓的 `unsafe`**:确需时必须写注释说明为何安全。
- 保持简单、别过度设计。改动尽量小而聚焦。

### 前端 / Frontend(apps/desktop)

- 类型检查:`pnpm -C apps/desktop exec tsc --noEmit` 必须通过。
- 构建:`pnpm -C apps/desktop build`(= `tsc && vite build`)必须通过。
- 遵循现有的组件与样式约定(React 19 + Tailwind v4);面向普通用户(含老年人),
  文案用「门诊/住院/手术」这类看得懂的词。

### 测试 / Tests

- **提 PR 前测试必须全绿。** 至少运行你改动相关的 crate:
  ```bash
  cargo test -p core-model -p parser -p pipeline -p ocr -p dicom -p medme-share -p medme-cli
  ```
- 新功能/修 bug 尽量带上测试。参考 [TESTING.md](TESTING.md) 了解人工验收要点。

### 隐私 / Privacy

- **绝不**提交真实敏感病历或个人健康信息。用 `examples/` 里的示例数据或脱敏文件。

---

## 提交与 PR / Commits and pull requests

- **提交信息**建议用 [Conventional Commits](https://www.conventionalcommits.org)
  风格:`type(scope): 摘要`,例如
  - `feat(dicom): 支持 JPEG-LS 解码`
  - `fix(pipeline): 修复重复导入时的去重判定`
  - `docs: 补充加密分享说明`
  说清**做了什么 + 为什么**。
- **流程**:从 `main` 拉分支 → 改动 → 本地跑 fmt / clippy / test(前端另加 tsc +
  build)→ 提 PR,填写 PR 模板 → 关联 issue → 等待评审。
- 保持 PR 小而聚焦,便于评审。
- **不要改动 `apps/mobile/**`**,除非该 PR 本身就是关于手机端。

---

## 有问题?/ Questions?

- 用法 / 找不到入口:先看官网 <https://chesterguan.github.io/medme/> 与
  [README](README.md)、[TESTING.md](TESTING.md)。
- 功能建议 / bug:开一个 [issue](https://github.com/Chesterguan/medme-app/issues)
  (用对应模板)。
- 安全问题:**私下**邮件 `chesterfield199512@gmail.com`(见 [SECURITY.md](SECURITY.md))。

本项目以 [Apache-2.0](LICENSE) 授权;提交贡献即表示你同意以该许可证发布你的贡献。
