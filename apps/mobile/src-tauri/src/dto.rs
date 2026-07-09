//! Serde DTOs sent to the React layer. A trimmed subset of the desktop app's
//! `dto.rs` — mobile milestone 1 only needs the 健康档案 timeline, ingest
//! outcomes, the patient header and share results.
use core_model::{Document, Encounter, SourceFile};
use serde::Serialize;

#[derive(Serialize)]
pub struct DocumentSummary {
    pub id: i64,
    pub doc_type: String,
    pub doc_date: Option<String>,     // RFC3339
    pub doc_date_end: Option<String>, // RFC3339
    pub title: Option<String>,
    pub page_count: i32,
    /// 影像检查文档的切片数;非影像文档为 None。
    pub slice_count: Option<i32>,
}
impl From<&Document> for DocumentSummary {
    fn from(d: &Document) -> Self {
        DocumentSummary {
            id: d.id,
            doc_type: d.doc_type.as_str().to_string(),
            doc_date: d.doc_date.map(|x| x.to_rfc3339()),
            doc_date_end: d.doc_date_end.map(|x| x.to_rfc3339()),
            title: d.title.clone(),
            page_count: d.page_count,
            slice_count: None,
        }
    }
}

#[derive(Serialize)]
pub struct EncounterSummary {
    pub id: i64,
    pub kind: String, // inpatient|outpatient|emergency|exam
    pub provider: Option<String>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub title: Option<String>,
    pub transferred: bool,
    pub doc_count: i64,
}
impl EncounterSummary {
    pub fn from_encounter(e: &Encounter, doc_count: i64) -> Self {
        EncounterSummary {
            id: e.id,
            kind: e.kind.as_str().to_string(),
            provider: e.provider.clone(),
            start_date: e.start_date.map(|x| x.to_rfc3339()),
            end_date: e.end_date.map(|x| x.to_rfc3339()),
            title: e.title.clone(),
            transferred: e.transferred,
            doc_count,
        }
    }
}

/// list_archive 返回的分组:就诊组 或 独立文档(与桌面 list_timeline_grouped 同构)。
#[derive(Serialize)]
#[serde(tag = "group_type")]
pub enum TimelineGroup {
    #[serde(rename = "encounter")]
    Encounter {
        encounter: EncounterSummary,
        docs: Vec<DocumentSummary>,
    },
    #[serde(rename = "document")]
    Document { doc: DocumentSummary },
}

/// 原始文件元信息(文档详情页展示来源 + 前端据此判断是否为图片以渲染缩略)。
#[derive(Serialize)]
pub struct SourceFileMeta {
    pub id: i64,
    pub original_name: String,
    pub mime_type: String,
    pub byte_size: i64,
    pub imported_at: String,
}
impl From<&SourceFile> for SourceFileMeta {
    fn from(s: &SourceFile) -> Self {
        SourceFileMeta {
            id: s.id,
            original_name: s.original_name.clone(),
            mime_type: s.mime_type.clone(),
            byte_size: s.byte_size,
            imported_at: s.imported_at.to_rfc3339(),
        }
    }
}

/// 文档详情:类型/日期(在 document 里)+ 来源文件 + 识别文本(与桌面同构,
/// 但移动端不含 DICOM 阅片,影像只显示元信息/缩略)。
#[derive(Serialize)]
pub struct DocumentDetail {
    pub document: DocumentSummary,
    pub source_file: SourceFileMeta,
    pub ocr_text: String,
    pub ocr_confidence: Option<f32>,
    pub ocr_backend: Option<String>,
}

#[derive(Serialize)]
pub struct ImportOutcome {
    pub name: String,
    pub source_file_id: i64,
    pub status: String, // new|backfilled|deduped|stored_no_text|instance_attached|failed
    pub doc_type: Option<String>,
}

/// 加密分享生成结果:口令(单独告知医生)、记录数、文件字节数、分享文件路径。
#[derive(Serialize)]
pub struct ShareResult {
    pub passphrase: String,
    pub record_count: i64,
    pub byte_size: i64,
    pub path: String,
}

#[derive(Serialize)]
pub struct PatientProfile {
    pub name: Option<String>,
    pub gender: Option<String>,
    pub birth_date: Option<String>,
    pub age: Option<String>,
    pub record_count: i64,
}
