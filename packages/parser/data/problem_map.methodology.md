# 问题导向临床分组表 — 方法与分组依据

**版本**: 草案 v1 · 2026-07-14 · MedMe（医我）

## 方法（一句话）
药物→疾病一律以 **ATC（解剖-治疗-化学）分类**的类别前缀为依据（ATC 前缀即引用）；实验室→疾病一律以**具名临床指南（含年份/版本）或标准定义**为依据；两者都**不做任何臆造**，无法找到可核对来源的映射一律省略。LOINC/ATC 编码直接复用本仓库 `packages/terminology/dictionary.json` 中已从本地 OMOP 词表（LOINC/RxNorm/ATC + OMOP standard concept_id）核验过的编码。

## 分组依据（Bibliography，可在 App 中向医生展示）

**药物分类标准**
- WHO Collaborating Centre for Drug Statistics Methodology（WHOCC）：**ATC/DDD Index**。相关一级/二级组：
  - A10 Drugs used in diabetes（A10A Insulins and analogues；A10B Blood glucose lowering drugs, excl. insulins：A10BA 双胍、A10BB 磺脲、A10BF α-糖苷酶抑制剂、A10BG 噻唑烷二酮、A10BH DPP-4、A10BJ GLP-1、A10BK SGLT2、A10BX 格列奈等）
  - C09 Agents acting on the renin-angiotensin system（C09A ACEI、C09C ARB）
  - C10 Lipid modifying agents（C10AA 他汀、C10AB 贝特、C10AX 其他含依折麦布/PCSK9）
  - C08 Calcium channel blockers（C08CA 二氢吡啶）、C07 Beta blocking agents、C03 Diuretics（C03A 噻嗪、C03B 噻嗪样、C03D 醛固酮拮抗剂）、C01DA Organic nitrates
  - B01 Antithrombotic agents（B01AC 抗血小板）、B03 Antianemic preparations（B03A 铁剂、B03B B12/叶酸、B03XA EPO）
  - M04 Antigout preparations（M04AA 抑制尿酸生成、M04AB 促排泄、M04AC 秋水仙碱）
  - H03 Thyroid therapy（H03A 甲状腺制剂、H03B 抗甲状腺药）

**实验室分组指南**
- 《中国糖尿病防治指南（2024版）》，中华医学会糖尿病学分会（2020年版《中国2型糖尿病防治指南》的最新修订，已更名）。
- 《中国高血压防治指南（2024年修订版）》，中华高血压杂志 2024年第7期。
- 《中国血脂管理指南（2023年）》，中华心血管病杂志 2023;51(3)（2016年版《中国成人血脂异常防治指南》的修订更名）。
- KDIGO 2024 Clinical Practice Guideline for the Evaluation and Management of Chronic Kidney Disease（GFR G1–G5 / 白蛋白尿 A1–A3 分级）；《中国慢性肾脏病早期评价与管理指南》。
- 《甲状腺功能减退症基层诊疗指南（2019年）》，中华全科医师杂志 2019;18(11)。
- 《中国甲状腺功能亢进症和其他原因所致甲状腺毒症诊治指南（2022）》，中华医学会内分泌学分会等。
- WHO. Haemoglobin concentrations for the diagnosis of anaemia and assessment of severity（2011，WHO/NMH/NHD/MNM/11.1）——贫血 Hgb 定义切点。
- 《中国高尿酸血症与痛风诊疗指南（2019）》，中华医学会内分泌学分会。
- 《代谢相关（非酒精性）脂肪性肝病防治指南（2024年版）》，中华医学会肝病学分会（2018更新版《非酒精性脂肪性肝病防治指南》的修订更名）。

## 覆盖情况与坦白说明
- **共覆盖 10 个疾病条目**（甲状腺功能异常按甲减/甲亢拆为两条，因 ICD 与药物类别不同）：58 条实验室映射、37 条药物映射。
- **grounding 扎实**：2型糖尿病、高血压、高脂血症、CKD、甲减/甲亢、痛风/高尿酸——labs 有对应中国/国际指南章节，drugs 有干净的 ATC 前缀。
- **thin / 需注意的条目**：
  - **冠心病**：慢性冠心病缺少一份可核对的"实验室监测"中国指南；LDL-C 借《中国血脂管理指南（2023）》ASCVD 二级预防章节 grounding 扎实，但**肌钙蛋白 I/T、CK-MB、NT-proBNP 属急性冠脉综合征/心衰的事件性检查，非慢性监测项**，已在各行 source 中明确标注，未伪装成慢病监测指南。药物侧（抗血小板/他汀/β阻滞剂/ACEI-ARB/硝酸酯）ATC grounding 完整。
  - **贫血**：贫血是综合征而非单一病，Hgb 定义切点用 WHO 2011；铁蛋白/血清铁/B12/叶酸属**病因鉴别 workup panel**（缺铁性/巨幼细胞性），已在 source 中如实标注为"病因诊断"而非单一指南强制项。
  - **代谢相关脂肪性肝病**：labs（ALT/AST/GGT + FIB-4 相关的血小板）grounding 扎实；但**无任何以脂肪肝为适应证的 ATC 药物类别**——2024 指南的药物建议均指向代谢共病（二甲双胍/SGLT2/GLP-1/他汀），因此 `drugs` 留空，不臆造"保肝药"类别。
- **省略项**：未纳入无干净 ATC 类别或无具名指南来源的映射（如各类"保肝药""改善微循环药"）。
