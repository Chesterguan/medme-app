# MedMe · 给 AI agent 的项目指针

MedMe(医我):**on-device、E2E 加密**的个人病历保险箱。导入照片/PDF → OCR → 分类 → 结构化抽取 → 医生查看器 summary → 加密分享。桌面 Tauri + 移动 Flutter,复用 Rust core。

## Session 开头:先重建上下文(别凭记忆)
1. **读 `docs/ADR/`**(架构决策,Nygard 格式;ADR 不可变,新决策加新号 supersede 旧号)。
2. **读 `docs/log/` 最新几条**(工程日志:讨论/测试数字/发现;精炼+链接)。
3. **按任务 grep `docs/ADR` + `docs/log`** 取相关切片(检索,不全读)。
4. 我的跨会话记忆在 `.claude/projects/*/memory/`(MEMORY.md 索引)。

**技术(设计/架构/测试/决策)→ git(ADR + log);商业/roadmap/产品 → Notion(除非用户关心)。** 公开 build-in-public blog 从 `docs/log/` 提炼(见最新 log)。

## 易忘、易记混的事实(读代码为准)
- **OCR 是各平台原生,不是单一引擎**:桌面 mac=Apple Vision / Win=Windows.Media.Ocr / Linux=PP-OCRv5;移动 iOS=Vision、Android=ML Kit(Dart 层,不走 Rust ocr crate)。见 [ADR 0005](docs/ADR/0005-ocr-per-platform-native.md)。**PP-OCRv5 ≠ 移动端引擎。**
- **抽取当前是正则**(`parser::assemble_summary`,分享时跑);MedGemma 只探索过、**0 集成**(#150/#157)。
- 存储是**事件溯源**(core-model:append_event + materialize + CAS);vault 格式须与桌面逐字节兼容——加事件类型 = 动格式,慎。
- 移动端 build 纪律见 `apps/mobile_flutter/CLAUDE.md`(不日常跑 release/全 ABI)。

## 工作纪律
- 提方案/结论前**先读代码**,别猜、别拿没验证的当事实、别在「好/不好」间摇摆(见 memory `verify-before-asserting`)。
- 性能测试用 `--release`。医疗数据:只输出原文逐字内容,逐字子串校验挡幻觉。
