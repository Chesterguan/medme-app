use core_model::{AuditEntry, Document, Encounter, ImagingInstance, SourceFile};
use serde::Serialize;

#[derive(Serialize)]
pub struct DocumentSummary {
    pub id: i64,
    pub doc_type: String,
    pub doc_date: Option<String>,     // RFC3339
    pub doc_date_end: Option<String>, // RFC3339
    pub title: Option<String>,
    pub page_count: i32,
    /// 影像检查文档的切片数(imaging overhaul P1);非影像文档为 None。
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

#[derive(Serialize)]
pub struct SearchResult {
    pub document: DocumentSummary,
    pub snippet: String,
}

/// 一台影像检查的一张切片(imaging overhaul P1),按堆栈顺序返回给前端逐张加载。
#[derive(Serialize)]
pub struct ImagingInstanceDto {
    pub source_file_id: i64,
    pub series_uid: Option<String>,
    pub series_number: Option<i32>,
    pub instance_number: Option<i32>,
}
impl From<&ImagingInstance> for ImagingInstanceDto {
    fn from(i: &ImagingInstance) -> Self {
        ImagingInstanceDto {
            source_file_id: i.source_file_id,
            series_uid: i.series_uid.clone(),
            series_number: i.series_number,
            instance_number: i.instance_number,
        }
    }
}

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
    pub status: String, // new|backfilled|deduped|stored_no_text
    pub doc_type: Option<String>,
}

#[derive(Serialize)]
pub struct ExportSummary {
    pub file_count: i64,
    pub byte_size: i64,
}

/// 加密分享生成结果:口令(分组显示)、记录数、文件字节数。
#[derive(Serialize)]
pub struct ShareResult {
    pub passphrase: String,
    pub record_count: i64,
    pub byte_size: i64,
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

#[derive(Serialize)]
pub struct PatientProfile {
    pub name: Option<String>,
    pub gender: Option<String>,
    pub birth_date: Option<String>,
    pub age: Option<String>,
    pub record_count: i64,
}

/// 审计追踪条目(隐藏的「审计/管理员」视图):时间 · 动作 · 详情 · 哈希 · 设备。
#[derive(Serialize)]
pub struct AuditEntryDto {
    pub seq: i64,
    pub timestamp: String,
    pub device_id: String,
    pub action: String,
    pub detail: String,
    pub sha256: Option<String>,
}
impl From<&AuditEntry> for AuditEntryDto {
    fn from(e: &AuditEntry) -> Self {
        AuditEntryDto {
            seq: e.seq,
            timestamp: e.timestamp.clone(),
            device_id: e.device_id.clone(),
            action: e.action.clone(),
            detail: e.detail.clone(),
            sha256: e.sha256.clone(),
        }
    }
}
