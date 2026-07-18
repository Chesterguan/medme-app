# 2026-07-18 · 移动端逐模块审计 + 抽取策略 + 竞品/blog 讨论

> 工程日志(build-in-public 内部源)。精炼 + 链接,不重讲 git/issue。清空上下文后先读这条 + `docs/ADR/` 重建。

## 做了什么(分支 `fix/mobile-ocr-and-viewer-audit`,未合并)
逐模块审计「导入→识别→分类→抽取→分享→查看器」,修复 + 落地 Layer-0/1。两个提交 `ff4950d`、`ac7b940`(+ 未提交:README OCR 事实修正、本 log/ADR)。

- **模块3 图片OCR**:安卓 ML Kit OCR 失败**不再丢照片**(加 catch,与 iOS 对齐);失败给用户干净提示。
- **模块4 扫描PDF**:移动端 OCR 引擎关(见 [ADR 0005](../ADR/0005-ocr-per-platform-native.md)),故 Flutter 用 `pdfx` 逐页渲染 PNG → 原生图片 OCR → `backfill_pdf_text` 回填。doc_type 用 OCR 文本重分类留 #155(方案C)。
- **模块5 分类**:病理报告级标记优先于「检验/化验/影像」字样(免误分)。
- **模块7 summary**:挂载门槛从「只认诊断」放宽为「诊断/趋势/影像/病理/过敏任一非空」(share.rs);查看器 gate 同步放宽。
- **模块9 查看器**:修先前就有的字段 bug(buildEMR 读错 `i.finding`/`i.name`,实际 `studies.finding`/`pathology.conclusion` → 复制文本里影像/病理一直空);加病理可视区块。
- **Layer-0 A**(OCR 保块结构,#分块 第0层):Android 保 ML Kit blocks;iOS Vision 排序+分块(Swift 未 headless 编译验证)。块边界 `\n\n` 目前 Layer-1 未消费(ponytail 记的空转,用户接受留着)。
- **Layer-1**(按 section header 分段路由,#148):`aggregate.rs` 新增 `SecKind`/`header_kind`/`sections_text`。lab_report/prescription 整文抽取不变;其它文档**额外**从内嵌「化验/用药段」抽 → 出院小结 4 个内嵌带药捡回(测试证)。

## 关键实测数字(别再瞎猜)
- **MedGemma-4B 零样本**:CMeEE 黄金 micro F1=0.52(out-of-domain,不代表我们场景);我们 14 份真实文档自标 gold:relaxed 药 F1=0.97、疾病 P/R=1.0(结构感知后)、化验被同义词匹配拖低(terminology 归一在产品里解决)。
- **OCR**:PP-OCRv5(桌面)release 1.3s/张(debug 曾误报 36s);真机中文照片置信度 0.36–0.41(Apple Vision)vs PP-OCRv5 0.69–0.95;PP-OCRv5 数值更准(读对 AV 读错的 43.60/4.35)。**但 PP-OCRv5 不是移动端引擎**(桌面/CLI)。
- **VLM 直吃图**:MedGemma 对中文化验照片**幻觉编造项目名**,排除。→ 走 OCR→文本→抽取。

## 抽取策略(已定方向)
**混合、不二选一**:OCR 保布局(Layer-0)→ 按 section header 规则分段(Layer-1)→ **散文块**才上小模型抽术语(Layer-2,未做)→ terminology 归一。第2层**不上 4B 零样本**,走**自己 LoRA 微调 0.5–1B**(#157)。分块是成熟领域(SecTag/MedSpaCy sectionizer 规则 99.5% + DLA),不是新问题。

## 竞品/定位(2026-07-18 评估)
- 差异化 = on-device + E2E 私密 + 跨院整合 + 能读懂。抽取本身**不新鲜**(腾讯/百度云结构化更强),不是护城河。
- **真正特别 = 确定性「跨院红旗情报」**(药物冲突/漏查/重复,#158)——不需云/LLM(本地药库+指南表),单院系统结构上做不到。难在数据采集。
- 查看器**已有**:趋势时间轴、可信度/证据链、status_note 解读、跨院 encounters。**真没有**:红旗(#158)。

## 开的 issue
#148(分段抽取,Layer-1 已部分落地)· #150(NER/MedGemma)· #154(terminology/WS446)· #155(扫描PDF重分类)· #156(国内国标 WS445/446/病案首页/ICD)· #157(LoRA 小模型)· #158(红旗面板)· #153(Dependabot 依赖升,含 rusqlite SQL注入修复 + 多个 major 跳,需测)。

## 待办 / 未完
- 公开 build-in-public blog:落地页在 **`gh-pages/index.html`**(直接在 gh-pages 维护,有 changelog.html 子页 + 动态版本号)。方案:gh-pages 加 `blog.html` + 定时 workflow 从本 log 提炼周报(AI 起草→人审→发)。**未做。**
- agentmemory 采用(未装)。
- iOS TestFlight 构建已手动触发(workflow_dispatch)供真机测。

## 会话结束状态(2026-07-18,记录后用户结束 session)
- **iOS TestFlight 构建 completed SUCCESS**(run 29622745822,10m25s,分支 `fix/mobile-ocr-and-viewer-audit`)→ 可真机测:导入 `scratch_test_pdfs/` 里 3 个无文本层扫描 PDF,看是否「已识别入库(扫描件)」。
- **分支清理**:从 ~70 删到 8。删了 63 个(**都确认内容在 main**:8 个空 diff + `fix/141`(#149 squash)+ 55 个有已合并 PR)。**保留** `main`/`lexuan/main`/`dependabot-cargo`(#153)/`fix/mobile-ocr-and-viewer-audit`(本会话)/ 4 个旧移动屏幕分支(`flutter/screen-*`、`fix/mobile-empty-state-copy`——无 PR 确认、疑似已被 main 重做取代,**按安全未删**,待确认)。
- **上下文重建地基已提交**(a7bcda6):ADR 0005 + 本 log + 根 CLAUDE.md + README 修正。下会话读 `CLAUDE.md → docs/ADR → docs/log` 重建。
- 本会话分支 `fix/mobile-ocr-and-viewer-audit` 含 3 个新提交(ff4950d/ac7b940/a7bcda6)+ 3 个 #141 旧提交(已在 main via #149 squash)→ 未来开 PR 前需 rebase 到 origin/main 去重。
- **决策**:采用 agentmemory(未装);公开 blog 中英**双语**;技术→git(ADR+log)/商业→Notion;抽取走混合(OCR块→段规则→LoRA 小模型 #157),不上 4B 零样本;红旗面板 #158 确定性、不需云。
- **待办(新会话做)**:#4 公开 blog(gh-pages blog.html 双语 + 周更 workflow 从本 log 提炼)· #5 依赖升级 #153(major 跳,需全测,清醒时做)· 4 个保留分支确认后删 · agentmemory 安装。
