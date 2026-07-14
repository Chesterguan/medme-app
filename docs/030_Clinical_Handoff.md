# 030 · Clinical Handoff · 患者掌控的临床交接层

> **状态(2026-07-14)**:方向已定,但**医生 summary 的 UI 结构在保留状态** —— 需先把
> 「国内医生标准套路」(《病历书写基本规范》/ 住院病案首页)与 IPS 对齐后再定版式(见 §3
> 会重写)。**已 spin-off 的活 = 术语归一覆盖率**(见对应 issue):`terminology` 已加确定性
> 药名剥壳,化验名候选拆分待落地,字典扩容到 ≥98% 是主要工作。抽取一律**确定性、无 LLM**;
> LLM 仅可能用于「生成 summary 的组织」且**数字由确定性来源填**(见 §5,已收回服务器 LLM 方案)。



> 战略「第二句话」:第一句是**所有权**——「你的病历只属于你,存在你自己的保险箱里」;
> 第二句是**有用性**——「需要时,它替你把病情讲清楚,并把医生的新判断带回来」。
> 本文是把第二句从口号落成技术方案的设计文档。产品定位与竞品判断见对话记录 2026-07-14。

关联:[011_Storage_Sync](011_Storage_Sync.md)(CAS+日志+E2E 分享)· [015_Trends_and_Timelines](015_Trends_and_Timelines.md) · 记忆 `medme-v12-scope`

---

## 1. 现状 vs 目标

- **现状**:加密分享包 = `{"records":[…逐份文档…]}`,查看器(`web/hosted-viewer` + `packages/share` 内嵌 HTML)把每份文档用内容感知渲染出来。= **给医生看一堆报告**。
- **目标**:在文档列表**之上**加一个**医生首屏摘要**(problem list / 当前用药 / 过敏 / 关键化验+趋势 / 近期就诊),每条**能下钻到源文档**(证据回溯)。= **医生 10 秒看懂病情上下文**。
- **不变量**:证据回溯、E2E(服务器永不见明文/密钥)、患者可撤销 —— 是第二句能不能立住的三条命脉。

## 2. 分三阶段(每阶段独立可交付)

| 阶段 | 内容 | 需要后端? |
|---|---|---|
| **A · 查看器 summary** | 查看器渲染 summary 首屏 + 证据下钻;summary 走**现有自包含加密分享**(发文件+口令) | ❌ 纯客户端 |
| **B · 结构化提取** | app 侧把 OCR 文本抽成结构化临床事实(OMOP-lite 事件),物化出 summary 打进分享包 | ❌(LLM 可选,见 §5) |
| **C · 小服务器** | 发**链接**代替发文件 + **真过期 / 撤销 / 访问日志**(#58) | ✅ 但只存密文 |

**关键:A、B 零后端。** 服务器(C)只买「链接/过期/撤销/日志」四样,是第二阶段,等 summary 跑通再上,顺便去风险。

## 3. 数据契约(THE artifact):doctor-summary JSON

分享包在现有 `records` 之外新增 `summary` 字段(缺省则查看器回退到纯文档列表,老分享不受影响)。`evidence` 是 `records` 数组的下标,点击跳到源文档 → 证据回溯白拿。

```jsonc
{
  "records": [ /* 现有:逐份文档(doc_type, ocr text, dicom…) */ ],
  "summary": {
    "generated_at": "2026-07-14T…",
    "patient": { "name": "张建国", "gender": "男", "age": "58" },
    "problems": [
      { "term": "2型糖尿病", "since": "2023", "evidence": [3, 7] }
    ],
    "medications": [
      { "name": "二甲双胍", "dose": "0.5g bid", "status": "active",
        "since": "2024-01", "note": null, "evidence": [7] },
      { "name": "阿司匹林", "dose": "100mg qd", "status": "stopped",
        "since": "2023-06", "note": "2024-03 停用", "evidence": [2, 5] }
    ],
    "allergies": [
      { "substance": "青霉素", "reaction": "皮疹", "evidence": [3] }
    ],
    "key_labs": [
      { "analyte": "HbA1c", "unit": "%",
        "latest": { "value": "7.2", "date": "2026-06", "flag": "high" },
        "trend": [ {"date":"2025-12","value":"7.8"}, {"date":"2026-06","value":"7.2"} ],
        "evidence": [9] }
    ],
    "recent_encounters": [
      { "date": "2026-06-15", "provider": "协和内分泌", "kind": "门诊", "evidence": [9] }
    ],
    "notable_changes": [ "近半年 HbA1c 7.8→7.2", "阿司匹林 2024-03 停用" ]
  }
}
```

- **status = active|stopped|unknown**:当前用药靠对 drug 事件按日期 fold(见 §4)得出;冲突(停了又用)落在 `note`。
- **不做**:标准词表编码(RxNorm/LOINC 概念 id)。医生看中文药名/指标名即可;编码是去重聚合 + Prometheno 互通用的,推后(§6)。

## 4. OMOP-lite 结构层(阶段 B 的地基)

采用 OMOP 的**领域形状**、跳过标准词表。每类事实建成**事件**进现有 append-only 日志(白拿证据回溯 + 跨设备合并 + 冲突物化):

| OMOP 域 | MedMe 事件载荷 |
|---|---|
| drug_exposure | {药名, 剂量, 频次, 动作(start/continue/stop), 日期, source_file_hash} |
| measurement | {指标, 值, 单位, 参考范围, 高低标记, 日期, source} |
| condition | {诊断词, 日期, source} |
| observation | {物质, 反应?, source}(过敏等) |
| procedure | {名称, 日期, source} |
| visit | 复用现有 encounter |

**当前用药 = fold**:阿司匹林 start(d1)→ stop(d2)→ restart(d3) ⇒ `active(自 d3),note: d2 停`。**只有事件溯源能干净做,纯 AI 摘要做不到 —— 这是独有能力。**

## 5. 抽取:parser 先,LLM 后(要拍板的分叉)

- **化验**:本来就有表格解析 → **持久成结构事实**(不只显示)。走 parser。
- **诊断/过敏/病程**(叙述性)→ LLM 结构化抽取更稳。隐私分叉:端上小模型(纯本地/弱) vs 服务器 LLM(强/OCR 文本出设备)。
- **务实**:混合;且 **LLM 抽取只在「生成医生分享」那一刻跑**(用户已决定分享,是可接受一次 AI 调用的时机)。平时纯本地。
- **MVP 不引入 LLM**:先用 parser/启发式从现有 `records` 抽 用药+化验+过敏,证明管线。

## 6. 与 Prometheno 的桥

结构层做成 OMOP 形状(哪怕不带词表)→ 患者数据将来流进 Prometheno 的 OMOP testbed **天然兼容**。MedMe = 患者侧的 OMOP-lite 喂数端。第三句(整个就医网络的受控协作)靠第二句攒下的医生网络效应,现在不做。

## 7. 服务器边界(阶段 C,#58)

只干四件事,**E2E 不变量:只存密文,永不见密钥/明文**:
- 存加密分享包(密文 blob)→ 发链接代替发文件
- 真过期(到期删密文)· 撤销(患者一键删)· 访问日志(谁何时打开)
- 合规面最小:服务器上只有密文,PIPL/医疗数据暴露面 = 0 明文

后续:医生「建议更正」= 一个 proposed 事件回传,患者确认后写入 → 从「发病历」进化到「共同维护病历」(第二句最重的半句)。

## 8. 最小切片(现在就做)

**查看器先行,零后端、零 app 改动**:
1. 定死 §3 的 `summary` schema(本文即契约)。
2. 查看器加 `renderSummary(payload.summary)`:文档列表**之上**渲染首屏(问题/用药/过敏/关键化验),每条 `evidence` → 点击滚动到对应 `records[i]`。`summary` 缺省则完全回退现状(老分享不受影响)。
3. 用 sample payload 验 UX。
4. 跑通后:app 侧做阶段 B 的 parser 抽取 → 产出 `summary` 打进分享包;再上阶段 C 服务器。

顺序:**查看器(A)→ app 提取(B)→ 服务器(C)**。先证明医生觉得有用,再补撤销/链接。
