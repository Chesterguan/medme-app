# 010 · Imaging / DICOM · 影像与 DICOM

> CT/MRI/超声等放射影像应以 **DICOM(.dcm)** 存储与解析,而非普通图片。DICOM 自带结构化元数据,不靠 OCR 就能拿到类型/日期/机构,并天然解决"影像图从属于报告 / 一次检查多张图"的分组。

关联:[009_Encounter_Model](009_Encounter_Model.md) · [003_Core_Data_Model](003_Core_Data_Model.md) · 记忆 `medme-content-aware-rendering`

---

## 1. 为什么 DICOM

普通 JPG/PNG 只有像素;DICOM 文件 = **像素数据 + 丰富标签(tags)**。放射影像(CT/MR/US/CR/DX/DR)几乎都是 DICOM。它给我们:
- **免 OCR 的结构化元数据**:modality、检查日期、机构、患者、检查部位、描述。
- **天然分组键**:`StudyInstanceUID`(一次检查的所有序列/图像)、`AccessionNumber`(检查号,链接影像 ↔ 报告)、`SeriesInstanceUID`(序列)。这正是 encounter/附属模型([009](009_Encounter_Model.md))的标准来源。

## 2. 关键标签 → MedMe 模型映射

| DICOM tag | (group,elem) | 映射到 |
|---|---|---|
| Modality | (0008,0060) | doc_type=imaging_report;子型 CT/MR/US/CR/DX |
| StudyDate / StudyTime | (0008,0020/0030) | `document.doc_date` |
| StudyDescription | (0008,1030) | 标题 |
| BodyPartExamined | (0018,0015) | 标题/部位 |
| InstitutionName | (0008,0080) | encounter.provider |
| PatientName/Sex/Age | (0010,0010/0040/1010) | 病人档案 |
| **AccessionNumber** | (0008,0050) | **链接影像 ↔ 同检查的报告**(附属) |
| **StudyInstanceUID** | (0020,000D) | **一次检查的多张图归一组** |
| SeriesInstanceUID / InstanceNumber | (0020,000E/0013) | 序列/帧排序 |
| WindowCenter/Width | (0028,1050/1051) | 渲染窗宽窗位 |

## 3. 解析与渲染

- **解析**:Rust `dicom-rs`(`dicom-object`)读标签;`dicom-pixeldata` 取像素。
- **渲染(v0.1)**:取代表帧,应用 Modality LUT + 窗宽窗位(CT 有 Hounsfield,需窗位;默认取 tag 或按 modality 预设)→ 8-bit 灰度 → 可查看图像。多帧/序列 → 画廊(next)。
- **原文件永存**:.dcm 原件存 CAS(Raw Never Dies);渲染出的预览是派生、可重建。

## 4. Pipeline 集成

`.dcm` 走独立分支(不经 parser/OCR):
```
.dcm → dicom-rs 解析 tags → 构造 document(imaging, doc_date=StudyDate, provider=InstitutionName,
        title=StudyDescription/BodyPart) → 存 .dcm 于 CAS → 渲染预览
      → 记录 study_uid / accession(用于分组与报告链接)
```
- 需要给 `document`/`source_file` 存 DICOM 标识:加 `study_uid`、`accession`、`modality`(可放 document 或一个 `imaging_meta` 表)。分组时:同 `study_uid` 的多张图 → 一个影像事件;`accession` 匹配同就诊内的影像报告 → 图像附属于该报告。
- encounter 分组([009]):影像事件按 StudyDate 落入时间窗;`accession` 提供比时间更强的"影像↔报告"关联。

## 5. 阶段

- **v0.1(下一步实现)**:解析 DICOM 元数据 → imaging document(类型/日期/机构/部位)+ 存原件 + 渲染单帧预览;时间线可见、可查看。
- **接续**:多帧/序列画廊;窗宽窗位交互;`accession`/`study_uid` 驱动的"影像↔报告"附属分组;DICOM 导出。
- 依赖:`dicom-object`、`dicom-pixeldata`(+ `image` 编码预览)。

## 6. 测试数据

需要真实样本:公开 DICOM(如 dicom-rs 测试集、公共匿名样本)。生成合成 DICOM 亦可(dicom-rs 可写)。加入 `examples/` 的影像样本集,覆盖 CT/MR/US/CR 各 modality。
