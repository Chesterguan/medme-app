# 更新日志 / Changelog

本文件记录 MedMe(医我)的重要变更。
格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/),
版本遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

All notable changes are documented here, following
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
[Semantic Versioning](https://semver.org/).

## [Unreleased]

- 手机端(iOS)采集应用开发中,与桌面共享 Rust 内核与存储。

## [1.0] - 2026-07

首个完整版本:一个本地优先、零服务器的个人医疗数据保险箱(桌面端)。
First complete release: a local-first, zero-server personal medical-data vault (desktop).

### 新增 / Added

- **事件溯源存储内核**:追加式事件日志(`log/*.jsonl`)+ 内容寻址存储(CAS,
  sha256)作为唯一真相;SQLite(FTS5 中文分词)为可随时重建的派生缓存。删库可从
  日志零损失重建。按设备分段日志(per-device log segmentation),便于多端同步免冲突。
- **导入管线**:拖拽导入 PDF / 图片 / 文本,自动去重、原件永久保存;扫描件 PDF
  识别;**自动收件箱**——往指定文件夹放文件即自动入库(watch folder)。
- **OCR(PP-OCRv5)**:识别中文报告照片与扫描件,含去阴影 / 纠偏预处理;显示识别
  置信度,低置信主动警告。
- **生命时间线 / 就诊模型**:按就诊 / 住院 / 转院 / 手术自动聚合(时间为主键);
  智能日期提取、病人信息栏、中文全文搜索。
- **按类型富渲染**:化验→表格(↑↓着色)、处方→用药清单、病理 / 影像 / 出院→分节;
  原件查看器(图片灯箱、PDF 内嵌)。
- **影像 / DICOM**:DICOM 解析与元数据提取(按类型 / 日期 / 检查分组);
  **交互式 DICOM 阅片**(窗宽窗位 / 缩放 / 多帧);支持压缩传输语法
  (JPEG2000 / JPEG-LS,内置 OpenJPEG / CharLS 编解码)。
- **导出**:一键导出自包含 HTML,浏览器打印 / 另存为 PDF 交给医生。
- **端到端加密分享**:AES-256-GCM 加密成一个自包含网页,凭口令在**任意浏览器本地
  解密**查看,**零服务器**,默认 5 天有效期;配套
  [在线预览器](https://chesterguan.github.io/medme-viewer/)。
- **审计与安全**:不可篡改的审计追踪(导入 / 导出 / 分享均记内容哈希 sha256);
  设置页可查看审计事件;**数据安全引导**(FileVault + iCloud 高级数据保护);
  中英文软件声明。
- **一键示例数据(张建国)**:内置示例数据集,测试者装完 .dmg 即可体验,无需自备文件。

### 备注 / Notes

- 测试版 .dmg **未签名 / 未公证**;首次打开需在访达右键「打开」或
  `xattr -cr /Applications/MedMe.app` 放行。正式版将签名 + 公证。
- MedMe 是数据整理工具,**非医疗器械**,不提供医疗诊断或建议;一切以原件为准。

[Unreleased]: https://github.com/Chesterguan/medme-app/compare/v1.0...HEAD
[1.0]: https://github.com/Chesterguan/medme-app/releases/tag/v1.0
