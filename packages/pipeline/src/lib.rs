use chrono::{DateTime, Utc};
use core_model::{DocType, NewDocument, NewImagingInstance, NewOcr, OcrBackendKind, Vault};
use std::collections::HashMap;
use std::path::Path;

/// 单个文件导入体积上限(字节)。超过即拒绝,**在把整份文件读进内存之前**就返回
/// 错误 —— 否则一份几个 GB 的文件/畸形附件会在任何解析器跑起来之前就把进程 OOM。
/// 200MB 足以覆盖高分辨率照片、扫描 PDF 与常见单张 DICOM;超大 DICOM 序列本就按
/// 单张切片逐个导入,单文件不会撞上限。移动端 `ingest_bytes` 复用此常量校验 payload。
pub const MAX_INGEST_BYTES: u64 = 200 * 1024 * 1024;

fn is_pdf(path: &Path) -> bool {
    mime_for(path) == "application/pdf"
}

fn is_dicom(path: &Path) -> bool {
    mime_for(path) == "application/dicom"
}

/// Builds a readable title from DICOM tags: modality+body part is most
/// specific ("CT · 头部"), then StudyDescription, then modality alone,
/// falling back to the original filename if nothing else is present.
fn dicom_title(meta: &dicom::DicomMeta, name: &str) -> String {
    if let (Some(m), Some(b)) = (&meta.modality, &meta.body_part) {
        return format!("{m} · {b}");
    }
    if let Some(d) = &meta.description {
        return d.clone();
    }
    if let Some(m) = &meta.modality {
        return m.clone();
    }
    name.to_string()
}

/// A short, searchable summary line synthesized from DICOM tags — DICOM has
/// no OCR text, so this stands in as the document's `ocr_result` body.
fn dicom_summary(meta: &dicom::DicomMeta) -> String {
    let mut lines = vec!["DICOM 影像检查".to_string()];
    if let Some(m) = &meta.modality {
        let cn = match m.as_str() {
            "CT" => "CT",
            "MR" => "MRI",
            "US" => "超声",
            "CR" | "DX" | "DR" => "X线",
            "MG" => "钼靶",
            "PT" => "PET",
            "NM" => "核医学",
            _ => m.as_str(),
        };
        lines.push(format!("检查类型:{cn}({m})"));
    }
    if let Some(d) = meta.study_date.as_deref().and_then(|d| d.split('T').next()) {
        lines.push(format!("检查日期:{d}"));
    }
    if let Some(b) = &meta.body_part {
        lines.push(format!("检查部位:{b}"));
    }
    if let Some(desc) = &meta.description {
        lines.push(format!("检查描述:{desc}"));
    }
    if let Some(i) = &meta.institution {
        lines.push(format!("检查机构:{i}"));
    }
    if let Some(p) = &meta.patient_name {
        lines.push(format!("患者:{p}"));
    }
    lines.push(
        "(DICOM 影像文件,点击上方原件可进行窗宽窗位 / 缩放 / 序列滚动交互阅片。)".to_string(),
    );
    lines.join("\n")
}

/// 按 DICOM 标签建/挂 study 文档(Study→Series→Instance,见 docs/014_Imaging_Overhaul.md)。
/// 免 OCR:DICOM 自带结构化元数据(见 docs/010_Imaging_DICOM.md)。
///
/// 分组:同 `StudyInstanceUID` 的多张切片归入**一个** imaging_report 文档 ——
/// 第一张切片建文档(合成摘要 + study 级标题/日期),其后同 study 的切片仅 append
/// 一条 imaging_instance(不建新文档)。因此一台 200 层 CT = 1 张时间线卡而非 200 份。
fn add_dicom_document(
    vault: &Vault,
    sid: i64,
    name: &str,
    bytes: &[u8],
    deduped: bool,
) -> anyhow::Result<IngestOutcome> {
    let meta = dicom::parse_meta(bytes)?;

    // 去重:同一张切片(非 study 锚点、无自己的 document)再次导入时,靠
    // imaging_instance 判定已入库,避免重复挂载。锚点切片的再导入由上游
    // `has_document` 早已拦截。
    if deduped {
        if let Some(_doc_id) = vault.imaging_document_for_source(sid)? {
            return Ok(IngestOutcome {
                source_file_id: sid,
                name: name.to_string(),
                status: IngestStatus::Deduped,
                doc_type: Some(DocType::ImagingReport),
            });
        }
    }

    // 已有同 study 的文档 → 只挂切片,不建新文档。
    if let Some(study_uid) = meta.study_uid.as_deref() {
        if let Some(doc_id) = vault.document_id_for_study(study_uid)? {
            vault.add_imaging_instance(NewImagingInstance {
                document_id: doc_id,
                source_file_id: sid,
                study_uid: study_uid.to_string(),
                series_uid: meta.series_uid.clone(),
                series_number: meta.series_number,
                instance_number: meta.instance_number,
            })?;
            let doc_type = vault
                .document_by_id(doc_id)?
                .map(|d| d.doc_type)
                .unwrap_or(DocType::ImagingReport);
            return Ok(IngestOutcome {
                source_file_id: sid,
                name: name.to_string(),
                status: IngestStatus::InstanceAttached,
                doc_type: Some(doc_type),
            });
        }
    }

    // 该 study 的第一张切片(或无 study_uid 的单张)→ 建 study 文档。
    let doc_date: Option<DateTime<Utc>> = meta
        .study_date
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc));
    let title = dicom_title(&meta, name);
    let summary = dicom_summary(&meta);

    let doc = vault.add_document(NewDocument {
        source_file_id: sid,
        doc_type: DocType::ImagingReport,
        doc_date,
        doc_date_end: None,
        title: Some(title),
        language: None,
        page_count: 1,
    })?;
    vault.add_ocr(NewOcr {
        document_id: doc.id,
        page_no: 1,
        backend: OcrBackendKind::Native,
        model_version: "dicom-meta".into(),
        text: summary,
        confidence: None,
    })?;
    // 首张切片挂到新建的 study 文档上(顺带把 study_uid 落到文档,供后续切片查找)。
    if let Some(study_uid) = meta.study_uid.as_deref() {
        vault.add_imaging_instance(NewImagingInstance {
            document_id: doc.id,
            source_file_id: sid,
            study_uid: study_uid.to_string(),
            series_uid: meta.series_uid.clone(),
            series_number: meta.series_number,
            instance_number: meta.instance_number,
        })?;
    }
    let status = if deduped {
        IngestStatus::Backfilled
    } else {
        IngestStatus::New
    };
    Ok(IngestOutcome {
        source_file_id: sid,
        name: name.to_string(),
        status,
        doc_type: Some(doc.doc_type),
    })
}

/// 按文本层(txt / 已抽取文本的 PDF)建 document + ocr_result(Native 后端)。
fn add_text_layer_document(
    vault: &Vault,
    sid: i64,
    name: &str,
    e: parser::Extracted,
    deduped: bool,
) -> anyhow::Result<IngestOutcome> {
    let doc = vault.add_document(NewDocument {
        source_file_id: sid,
        doc_type: e.doc_type.clone(),
        doc_date: e.doc_date,
        doc_date_end: e.doc_date_end,
        title: Some(name.to_string()),
        language: e.language,
        page_count: e.page_count,
    })?;
    vault.add_ocr(NewOcr {
        document_id: doc.id,
        page_no: 1,
        backend: OcrBackendKind::Native,
        model_version: "text-layer".into(),
        text: e.text,
        confidence: None,
    })?;
    let status = if deduped {
        IngestStatus::Backfilled
    } else {
        IngestStatus::New
    };
    Ok(IngestOutcome {
        source_file_id: sid,
        name: name.to_string(),
        status,
        doc_type: Some(doc.doc_type),
    })
}

pub fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "tif" | "tiff" => "image/tiff",
        "dcm" => "application/dicom",
        _ => "application/octet-stream",
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum IngestStatus {
    New,
    Deduped,
    Backfilled,
    StoredNoText,
    /// DICOM 切片并入了已存在的同 study 影像检查文档(未新建文档)。
    InstanceAttached,
}

#[derive(Debug, Clone)]
pub struct IngestOutcome {
    pub source_file_id: i64,
    pub name: String,
    pub status: IngestStatus,
    pub doc_type: Option<DocType>,
}

/// 导入一个文件:存 CAS(去重)→ 若尚无 document 则抽文本层并建 document/ocr。
/// 抽取失败(如扫描图片)不致命 → StoredNoText(原文件已永存,留待后续 OCR 补索引)。
pub fn ingest(vault: &Vault, path: &Path) -> anyhow::Result<IngestOutcome> {
    // 体积闸门:先看元数据里的文件大小,超上限就拒绝 —— 绝不 slurp 一份不可信的
    // 超大文件进内存(那会在解析前就 OOM)。用 metadata().len() 而非读入后再判。
    let len = std::fs::metadata(path)?.len();
    if len > MAX_INGEST_BYTES {
        anyhow::bail!(
            "文件过大:{len} 字节,超过上限 {MAX_INGEST_BYTES} 字节(200MB),已拒绝导入 / file too large"
        );
    }
    let bytes = std::fs::read(path)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();
    let imp = vault.import(&name, mime_for(path), &bytes)?;
    let sid = imp.source_file.id;

    if imp.deduped && vault.has_document(sid)? {
        return Ok(IngestOutcome {
            source_file_id: sid,
            name,
            status: IngestStatus::Deduped,
            doc_type: None,
        });
    }

    // .dcm 走独立分支(不经 parser/OCR):DICOM 自带结构化元数据,免 OCR 即可
    // 拿到类型/日期/机构(见 docs/010_Imaging_DICOM.md)。
    if is_dicom(path) {
        return add_dicom_document(vault, sid, &name, &bytes, imp.deduped);
    }

    // 无文本层的判定阈值:去空白后 < 20 字符视为"实际没有文本层"(扫描图 PDF 常见,
    // pdf-extract 对纯图片页返回空/近空字符串,不报错)。
    const MIN_TEXT_LAYER_LEN: usize = 20;

    match parser::extract(path) {
        Ok(e) if is_pdf(path) && e.text.trim().len() < MIN_TEXT_LAYER_LEN => {
            // 扫描图 PDF(无文本层):尝试从页面图片 OCR 补文本,像图片一样处理。
            match ocr::recognize_pdf(&bytes) {
                Ok(outcome) if !outcome.text.trim().is_empty() => {
                    let text = outcome.text;
                    let doc_type = parser::classify(&text);
                    let (doc_date, doc_date_end) = parser::guess_date_range(&text);
                    let doc = vault.add_document(NewDocument {
                        source_file_id: sid,
                        doc_type: doc_type.clone(),
                        doc_date,
                        doc_date_end,
                        title: Some(name.clone()),
                        language: parser::detect_language(&text),
                        page_count: e.page_count,
                    })?;
                    vault.add_ocr(NewOcr {
                        document_id: doc.id,
                        page_no: 1,
                        backend: OcrBackendKind::Onnx,
                        model_version: "ppocr-v5-pdf".into(),
                        text,
                        confidence: Some(outcome.confidence),
                    })?;
                    let status = if imp.deduped {
                        IngestStatus::Backfilled
                    } else {
                        IngestStatus::New
                    };
                    Ok(IngestOutcome {
                        source_file_id: sid,
                        name,
                        status,
                        doc_type: Some(doc_type),
                    })
                }
                // OCR 失败/空:退回原有行为 —— 按抽取到的(近空)文本层建 document。
                _ => add_text_layer_document(vault, sid, &name, e, imp.deduped),
            }
        }
        Ok(e) => add_text_layer_document(vault, sid, &name, e, imp.deduped),
        Err(_) => {
            // 无文本层(图片/扫描件):先尝试 OCR。
            match ocr::recognize(&bytes) {
                Ok(outcome) if !outcome.text.trim().is_empty() => {
                    // OCR 成功:像文本文档一样处理(分类/日期取自识别文本)
                    let text = outcome.text;
                    let doc_type = parser::classify(&text);
                    let (doc_date, doc_date_end) = parser::guess_date_range(&text);
                    let doc = vault.add_document(NewDocument {
                        source_file_id: sid,
                        doc_type: doc_type.clone(),
                        doc_date,
                        doc_date_end,
                        title: Some(name.clone()),
                        language: parser::detect_language(&text),
                        page_count: 1,
                    })?;
                    vault.add_ocr(NewOcr {
                        document_id: doc.id,
                        page_no: 1,
                        backend: OcrBackendKind::Onnx,
                        model_version: "ppocr-v5".into(),
                        text,
                        confidence: Some(outcome.confidence),
                    })?;
                    let status = if imp.deduped {
                        IngestStatus::Backfilled
                    } else {
                        IngestStatus::New
                    };
                    Ok(IngestOutcome {
                        source_file_id: sid,
                        name,
                        status,
                        doc_type: Some(doc_type),
                    })
                }
                _ => {
                    // OCR 失败/空:退回文件名元数据(保持现状),原文件已永存,
                    // 使其在时间线可见、可查看原件。
                    let (doc_date, doc_date_end) = parser::guess_date_range(&name);
                    let doc_type = parser::classify(&name);
                    vault.add_document(NewDocument {
                        source_file_id: sid,
                        doc_type: doc_type.clone(),
                        doc_date,
                        doc_date_end,
                        title: Some(name.clone()),
                        language: None,
                        page_count: 1,
                    })?;
                    // 不建 ocr_result(暂无文本)
                    Ok(IngestOutcome {
                        source_file_id: sid,
                        name,
                        status: IngestStatus::StoredNoText,
                        doc_type: Some(doc_type),
                    })
                }
            }
        }
    }
}

pub struct PatientProfile {
    pub name: Option<String>,
    pub gender: Option<String>,
    pub birth_date: Option<String>,
    pub age: Option<String>,
    pub record_count: i64,
}

/// 从所有文档 OCR 文本派生病人档案:各字段取众数(最常出现值)。
/// 年龄随时间变,取众数为近似;身份靠姓名+性别(稳定)。
pub fn patient_profile(vault: &Vault) -> anyhow::Result<PatientProfile> {
    let texts = vault.all_ocr_texts()?;
    let record_count = texts.len() as i64;
    let mut names: HashMap<String, i32> = HashMap::new();
    let mut genders: HashMap<String, i32> = HashMap::new();
    let mut births: HashMap<String, i32> = HashMap::new();
    let mut ages: HashMap<String, i32> = HashMap::new();
    for t in &texts {
        let d = parser::extract_demographics(t);
        if let Some(n) = d.name {
            *names.entry(n).or_insert(0) += 1;
        }
        if let Some(g) = d.gender {
            *genders.entry(g).or_insert(0) += 1;
        }
        if let Some(b) = d.birth_date {
            *births.entry(b).or_insert(0) += 1;
        }
        if let Some(a) = d.age {
            *ages.entry(a).or_insert(0) += 1;
        }
    }
    let mode = |m: HashMap<String, i32>| m.into_iter().max_by_key(|(_, c)| *c).map(|(k, _)| k);
    Ok(PatientProfile {
        name: mode(names),
        gender: mode(genders),
        birth_date: mode(births),
        age: mode(ages),
        record_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::Vault;
    use std::io::Write;

    fn tmp_txt(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        p
    }

    #[test]
    fn ingest_new_then_dedup() {
        let vdir = tempfile::tempdir().unwrap();
        let fdir = tempfile::tempdir().unwrap();
        let v = Vault::open(vdir.path()).unwrap();
        let f = tmp_txt(
            fdir.path(),
            "report.txt",
            "出院记录 2023-05-01 肌酐 Creatinine 120",
        );

        let o1 = ingest(&v, &f).unwrap();
        assert_eq!(o1.status, IngestStatus::New);
        assert!(o1.doc_type.is_some());

        let o2 = ingest(&v, &f).unwrap();
        assert_eq!(o2.status, IngestStatus::Deduped); // 已存在且已索引
        assert_eq!(o1.source_file_id, o2.source_file_id);

        // 时间线只有一条
        assert_eq!(v.timeline().unwrap().len(), 1);
    }

    #[test]
    fn ingest_no_text_still_creates_visible_document() {
        let vdir = tempfile::tempdir().unwrap();
        let fdir = tempfile::tempdir().unwrap();
        let v = Vault::open(vdir.path()).unwrap();
        // 文件名带日期+影像关键词;内容无文本层(.png 扩展名 → parser 报错)
        let p = fdir.path().join("2025-09-01_胸部X线_扫描件.png");
        std::fs::write(&p, b"\x89PNG\r\n\x1a\nnot-a-real-image").unwrap();

        let o = ingest(&v, &p).unwrap();
        assert_eq!(o.status, IngestStatus::StoredNoText);
        // 现在建了 document → 时间线可见,类型/日期取自文件名
        assert!(v.has_document(o.source_file_id).unwrap());
        let tl = v.timeline().unwrap();
        assert_eq!(tl.len(), 1);
        assert_eq!(tl[0].doc_type, core_model::DocType::ImagingReport);
        assert_eq!(
            tl[0].doc_date.unwrap().format("%Y-%m-%d").to_string(),
            "2025-09-01"
        );
        // 无 OCR 文本
        assert_eq!(v.ocr_text(tl[0].document_id).unwrap(), "");
    }

    /// .dcm 走独立分支:元数据(非 OCR)驱动 doc_type/日期/标题,原文件+摘要
    /// 均可查。样本文件随仓库提交,读取本地路径,离线可跑。
    #[test]
    fn ingest_dicom_ct_builds_imaging_document() {
        let vdir = tempfile::tempdir().unwrap();
        let v = Vault::open(vdir.path()).unwrap();
        let p = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/demo-dataset/dicom/CT_small.dcm"
        ));
        let o = ingest(&v, p).unwrap();
        assert_eq!(o.status, IngestStatus::New);
        assert_eq!(o.doc_type, Some(core_model::DocType::ImagingReport));

        let tl = v.timeline().unwrap();
        assert_eq!(tl.len(), 1);
        assert_eq!(tl[0].doc_type, core_model::DocType::ImagingReport);
        assert_eq!(
            tl[0].doc_date.unwrap().format("%Y-%m-%d").to_string(),
            "2004-01-19"
        );

        let text = v.ocr_text(tl[0].document_id).unwrap();
        assert!(text.contains("CT"), "unexpected summary text: {text}");
        assert!(
            text.contains("JFK IMAGING CENTER"),
            "unexpected summary text: {text}"
        );

        // 去重再导入:不重复建 document,时间线仍只有一条
        let o2 = ingest(&v, p).unwrap();
        assert_eq!(o2.status, IngestStatus::Deduped);
        assert_eq!(v.timeline().unwrap().len(), 1);
    }

    /// 导入一个 12 张切片的 DICOM 序列文件夹 → 应聚成**一个** imaging_report 文档
    /// (而非 12 份),含 12 条有序 imaging_instance(按 instance_number)。再次导入
    /// 整个文件夹全部去重,文档与切片数均不变。样本随仓库提交,离线可跑。
    #[test]
    fn ingest_dicom_series_groups_into_one_study() {
        let vdir = tempfile::tempdir().unwrap();
        let v = Vault::open(vdir.path()).unwrap();
        let dir = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/demo-dataset/imaging/头颅CT序列"
        ));
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("dcm"))
            .collect();
        files.sort();
        assert_eq!(files.len(), 12, "fixture should have 12 slices");

        let mut new_count = 0;
        let mut attached_count = 0;
        for f in &files {
            let o = ingest(&v, f).unwrap();
            match o.status {
                IngestStatus::New => new_count += 1,
                IngestStatus::InstanceAttached => attached_count += 1,
                s => panic!("unexpected status {s:?} for {}", f.display()),
            }
            assert_eq!(o.doc_type, Some(core_model::DocType::ImagingReport));
        }
        // 第一张建文档,其余 11 张并入。
        assert_eq!(new_count, 1);
        assert_eq!(attached_count, 11);

        // 时间线只有一张卡。
        let tl = v.timeline().unwrap();
        assert_eq!(tl.len(), 1);
        let doc_id = tl[0].document_id;

        // 12 条切片,按 instance_number 有序(1..=12)。
        let insts = v.imaging_instances(doc_id).unwrap();
        assert_eq!(insts.len(), 12);
        let order: Vec<i32> = insts.iter().map(|i| i.instance_number.unwrap()).collect();
        assert_eq!(order, (1..=12).collect::<Vec<_>>());

        // 再次导入整个文件夹 → 全部去重,不新增文档/切片。
        for f in &files {
            let o = ingest(&v, f).unwrap();
            assert_eq!(
                o.status,
                IngestStatus::Deduped,
                "re-import should dedup {}",
                f.display()
            );
        }
        assert_eq!(v.timeline().unwrap().len(), 1);
        assert_eq!(v.imaging_instances(doc_id).unwrap().len(), 12);

        // rebuild_from_log 后状态一致(脱库重放也是一个 study + 12 切片)。
        v.rebuild_from_log().unwrap();
        assert_eq!(v.timeline().unwrap().len(), 1);
        assert_eq!(v.imaging_instances(doc_id).unwrap().len(), 12);
    }

    /// 单张 DICOM(带 study_uid、无同伴切片)仍工作:1 个 study 文档 + 1 条切片。
    #[test]
    fn ingest_single_dicom_is_one_study_one_instance() {
        let vdir = tempfile::tempdir().unwrap();
        let v = Vault::open(vdir.path()).unwrap();
        let p = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/demo-dataset/dicom/CT_small.dcm"
        ));
        let o = ingest(&v, p).unwrap();
        assert_eq!(o.status, IngestStatus::New);
        let tl = v.timeline().unwrap();
        assert_eq!(tl.len(), 1);
        let insts = v.imaging_instances(tl[0].document_id).unwrap();
        assert_eq!(insts.len(), 1);
        assert_eq!(insts[0].instance_number, Some(1));
    }

    /// 扫描图 PDF(无文本层):应通过 recognize_pdf 补 OCR 文本,分类/日期取自
    /// 识别文本,而非退回文件名。需要 OCR 模型(联网首次下载,之后缓存)。
    ///   cargo test -p pipeline -- --ignored
    #[test]
    #[ignore]
    fn ingest_scanned_pdf_ocrs_and_dates() {
        let vdir = tempfile::tempdir().unwrap();
        let v = Vault::open(vdir.path()).unwrap();
        let p = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/demo-dataset/photos/2026-03-15_检验报告_扫描图PDF.pdf"
        ));
        let o = ingest(&v, p).unwrap();
        assert_eq!(o.status, IngestStatus::New);
        assert_eq!(o.doc_type, Some(core_model::DocType::LabReport));
        let tl = v.timeline().unwrap();
        assert_eq!(tl.len(), 1);
        assert_eq!(
            tl[0].doc_date.unwrap().format("%Y-%m-%d").to_string(),
            "2026-03-15"
        );
        let text = v.ocr_text(tl[0].document_id).unwrap();
        assert!(
            text.contains("肌酐") || text.contains("Creatinine"),
            "unexpected OCR text: {text}"
        );
    }

    /// 超过体积上限的文件被拒绝:用 `set_len` 造一个稀疏文件(metadata 报大小
    /// > 上限,但磁盘上几乎不占空间、内存里一个字节都没读),确认 ingest 在
    /// `fs::read` 之前就凭 metadata 返回「文件过大」错误,而非 OOM。
    #[test]
    fn ingest_rejects_oversized_file_before_reading() {
        let vdir = tempfile::tempdir().unwrap();
        let fdir = tempfile::tempdir().unwrap();
        let v = Vault::open(vdir.path()).unwrap();
        let p = fdir.path().join("huge.pdf");
        let f = std::fs::File::create(&p).unwrap();
        // 稀疏:声明比上限多 1 字节的长度,不实际写入数据。
        f.set_len(MAX_INGEST_BYTES + 1).unwrap();
        drop(f);

        let err = ingest(&v, &p).unwrap_err();
        assert!(
            err.to_string().contains("文件过大"),
            "expected size-cap error, got: {err}"
        );
        // 什么都不该入库(既没读文件,也没建 document)。
        assert_eq!(v.timeline().unwrap().len(), 0);
    }

    #[test]
    fn ingest_captures_date_range() {
        let vdir = tempfile::tempdir().unwrap();
        let fdir = tempfile::tempdir().unwrap();
        let v = Vault::open(vdir.path()).unwrap();
        let p = fdir.path().join("discharge.txt");
        std::fs::write(
            &p,
            "出院记录\n入院日期:2023-01-01 出院日期:2023-01-20\n脑梗死",
        )
        .unwrap();
        ingest(&v, &p).unwrap();
        let tl = v.timeline().unwrap();
        assert_eq!(
            tl[0].doc_date.unwrap().format("%Y-%m-%d").to_string(),
            "2023-01-01"
        );
        assert_eq!(
            tl[0].doc_date_end.unwrap().format("%Y-%m-%d").to_string(),
            "2023-01-20"
        );
    }

    #[test]
    fn patient_profile_aggregates_mode() {
        let vdir = tempfile::tempdir().unwrap();
        let fdir = tempfile::tempdir().unwrap();
        let v = Vault::open(vdir.path()).unwrap();
        for (i, body) in [
            "检验报告\n姓名:张建国 性别:男 年龄:59岁\n日期 2024-01-01 肌酐 90",
            "出院记录\n姓名:张建国 性别:男 年龄:60岁\n日期 2025-02-02 脑梗死",
        ]
        .iter()
        .enumerate()
        {
            let p = fdir.path().join(format!("r{i}.txt"));
            std::fs::write(&p, body).unwrap();
            ingest(&v, &p).unwrap();
        }
        let prof = patient_profile(&v).unwrap();
        assert_eq!(prof.name.as_deref(), Some("张建国"));
        assert_eq!(prof.gender.as_deref(), Some("男"));
        assert_eq!(prof.record_count, 2);
    }
}
