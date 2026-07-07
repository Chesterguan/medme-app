use chrono::{DateTime, Utc};
use core_model::{DocType, NewDocument, NewOcr, OcrBackendKind, Vault};
use std::collections::HashMap;
use std::path::Path;

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
    let parts = [
        meta.modality.as_deref(),
        meta.study_date.as_deref().and_then(|d| d.split('T').next()),
        meta.description.as_deref(),
        meta.institution.as_deref(),
        meta.patient_name.as_deref(),
    ];
    parts.into_iter().flatten().collect::<Vec<_>>().join(" ")
}

/// 按 DICOM 标签建 document + ocr_result(Native 后端,合成摘要文本)。
/// 免 OCR:DICOM 自带结构化元数据(见 docs/010_Imaging_DICOM.md)。
fn add_dicom_document(
    vault: &Vault,
    sid: i64,
    name: &str,
    bytes: &[u8],
    deduped: bool,
) -> anyhow::Result<IngestOutcome> {
    let meta = dicom::parse_meta(bytes)?;
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
    let status = if deduped { IngestStatus::Backfilled } else { IngestStatus::New };
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
    let status = if deduped { IngestStatus::Backfilled } else { IngestStatus::New };
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
                Ok(text) if !text.trim().is_empty() => {
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
                        confidence: None,
                    })?;
                    let status = if imp.deduped {
                        IngestStatus::Backfilled
                    } else {
                        IngestStatus::New
                    };
                    Ok(IngestOutcome { source_file_id: sid, name, status, doc_type: Some(doc_type) })
                }
                // OCR 失败/空:退回原有行为 —— 按抽取到的(近空)文本层建 document。
                _ => add_text_layer_document(vault, sid, &name, e, imp.deduped),
            }
        }
        Ok(e) => add_text_layer_document(vault, sid, &name, e, imp.deduped),
        Err(_) => {
            // 无文本层(图片/扫描件):先尝试 OCR。
            match ocr::recognize(&bytes) {
                Ok(text) if !text.trim().is_empty() => {
                    // OCR 成功:像文本文档一样处理(分类/日期取自识别文本)
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
                        confidence: None,
                    })?;
                    let status = if imp.deduped {
                        IngestStatus::Backfilled
                    } else {
                        IngestStatus::New
                    };
                    Ok(IngestOutcome { source_file_id: sid, name, status, doc_type: Some(doc_type) })
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
                    Ok(IngestOutcome { source_file_id: sid, name, status: IngestStatus::StoredNoText, doc_type: Some(doc_type) })
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
        if let Some(n) = d.name { *names.entry(n).or_insert(0) += 1; }
        if let Some(g) = d.gender { *genders.entry(g).or_insert(0) += 1; }
        if let Some(b) = d.birth_date { *births.entry(b).or_insert(0) += 1; }
        if let Some(a) = d.age { *ages.entry(a).or_insert(0) += 1; }
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
        let f = tmp_txt(fdir.path(), "report.txt", "出院记录 2023-05-01 肌酐 Creatinine 120");

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
        assert_eq!(tl[0].doc_date.unwrap().format("%Y-%m-%d").to_string(), "2025-09-01");
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
        assert_eq!(tl[0].doc_date.unwrap().format("%Y-%m-%d").to_string(), "2004-01-19");

        let text = v.ocr_text(tl[0].document_id).unwrap();
        assert!(text.contains("CT"), "unexpected summary text: {text}");
        assert!(text.contains("JFK IMAGING CENTER"), "unexpected summary text: {text}");

        // 去重再导入:不重复建 document,时间线仍只有一条
        let o2 = ingest(&v, p).unwrap();
        assert_eq!(o2.status, IngestStatus::Deduped);
        assert_eq!(v.timeline().unwrap().len(), 1);
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
        assert_eq!(tl[0].doc_date.unwrap().format("%Y-%m-%d").to_string(), "2026-03-15");
        let text = v.ocr_text(tl[0].document_id).unwrap();
        assert!(text.contains("肌酐") || text.contains("Creatinine"), "unexpected OCR text: {text}");
    }

    #[test]
    fn ingest_captures_date_range() {
        let vdir = tempfile::tempdir().unwrap();
        let fdir = tempfile::tempdir().unwrap();
        let v = Vault::open(vdir.path()).unwrap();
        let p = fdir.path().join("discharge.txt");
        std::fs::write(&p, "出院记录\n入院日期:2023-01-01 出院日期:2023-01-20\n脑梗死").unwrap();
        ingest(&v, &p).unwrap();
        let tl = v.timeline().unwrap();
        assert_eq!(tl[0].doc_date.unwrap().format("%Y-%m-%d").to_string(), "2023-01-01");
        assert_eq!(tl[0].doc_date_end.unwrap().format("%Y-%m-%d").to_string(), "2023-01-20");
    }

    #[test]
    fn patient_profile_aggregates_mode() {
        let vdir = tempfile::tempdir().unwrap();
        let fdir = tempfile::tempdir().unwrap();
        let v = Vault::open(vdir.path()).unwrap();
        for (i, body) in [
            "检验报告\n姓名:张建国 性别:男 年龄:59岁\n日期 2024-01-01 肌酐 90",
            "出院记录\n姓名:张建国 性别:男 年龄:60岁\n日期 2025-02-02 脑梗死",
        ].iter().enumerate() {
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
