//! FRB 友好的 DTO,直接经 `flutter_rust_bridge` 生成对应 Dart class,供
//! `api::vault` 里的全量 vault API 使用。逐字段镜像 Tauri 移动端的
//! `apps/mobile/src-tauri/src/dto.rs`(同一批字段/同一批类型),只是去掉了
//! `serde::Serialize`——FRB 直接从这些 plain struct/enum 生成绑定,不经 JSON。
use core_model::{Document, Encounter, SourceFile};

/// iCloud 同步状态(设置页开关据此渲染)。`available` = 当前能否解析到 iCloud
/// 容器(iOS-only,由 Dart 侧经 `medme/icloud` MethodChannel 判断后覆盖;Rust
/// 恒返回 false);`enabled` = 本设备是否已开启同步(Rust 据持久标记返回)。
/// 开关/迁移逻辑见 `api::vault` 的 `enable_icloud_sync` / `disable_icloud_sync`。
#[derive(Debug, Clone)]
pub struct IcloudStatusDto {
    pub available: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct DocumentSummaryDto {
    pub id: i64,
    pub doc_type: String,
    pub doc_date: Option<String>,     // RFC3339
    pub doc_date_end: Option<String>, // RFC3339
    pub title: Option<String>,
    pub page_count: i32,
    /// 影像检查文档的切片数;非影像文档为 None。
    pub slice_count: Option<i32>,
}
impl From<&Document> for DocumentSummaryDto {
    fn from(d: &Document) -> Self {
        DocumentSummaryDto {
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

#[derive(Debug, Clone)]
pub struct EncounterSummaryDto {
    pub id: i64,
    pub kind: String, // inpatient|outpatient|emergency|exam
    pub provider: Option<String>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub title: Option<String>,
    pub transferred: bool,
    pub doc_count: i64,
}
impl EncounterSummaryDto {
    // `pub(crate)`, not `pub`: an inherent `pub fn` here would get picked up by FRB's
    // scanner as an exposed API method (it scans `crate::api` for pub symbols,
    // including inherent impl methods, not just free functions in `vault.rs`) and
    // then choke on `&Encounter` (a plain core-model type, not one of our mirrored
    // DTOs) as an unresolvable opaque type. `pub(crate)` keeps it a normal internal
    // helper, reachable from `api::vault`, invisible to codegen.
    pub(crate) fn from_encounter(e: &Encounter, doc_count: i64) -> Self {
        EncounterSummaryDto {
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

/// `load_archive` 返回的分组:就诊组 或 独立文档(与桌面/Tauri 移动端的
/// `TimelineGroup` 同构)。
#[derive(Debug, Clone)]
pub enum TimelineGroupDto {
    Encounter {
        encounter: EncounterSummaryDto,
        docs: Vec<DocumentSummaryDto>,
    },
    Document {
        doc: DocumentSummaryDto,
    },
}

/// 原始文件元信息(文档详情页展示来源 + 前端据此判断是否为图片以渲染缩略)。
#[derive(Debug, Clone)]
pub struct SourceFileMetaDto {
    pub id: i64,
    pub original_name: String,
    pub mime_type: String,
    pub byte_size: i64,
    pub imported_at: String,
}
impl From<&SourceFile> for SourceFileMetaDto {
    fn from(s: &SourceFile) -> Self {
        SourceFileMetaDto {
            id: s.id,
            original_name: s.original_name.clone(),
            mime_type: s.mime_type.clone(),
            byte_size: s.byte_size,
            imported_at: s.imported_at.to_rfc3339(),
        }
    }
}

/// 文档详情:类型/日期(在 document 里)+ 来源文件 + 识别文本。
#[derive(Debug, Clone)]
pub struct DocumentDetailDto {
    pub document: DocumentSummaryDto,
    pub source_file: SourceFileMetaDto,
    pub ocr_text: String,
    pub ocr_confidence: Option<f32>,
    pub ocr_backend: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ImportOutcomeDto {
    pub name: String,
    pub source_file_id: i64,
    pub status: String, // new|backfilled|deduped|stored_no_text|instance_attached|failed
    pub doc_type: Option<String>,
    /// 本次采集落库的文档 id(前端「待确认」review 队列据此显式标记新导入)。
    /// 去重/失败等没建文档的情况为 None。
    pub document_id: Option<i64>,
    /// 从本份报告文本里识别出的**患者姓名**(`parser::extract_demographics`)。
    /// 前端用它和当前成员档案名字比对——不一致就在「待确认」里标红警告(防导错人)。
    /// 识别不到为 None。
    pub detected_name: Option<String>,
}

/// 加密分享生成结果:口令(单独告知医生)、记录数、文件字节数、分享文件路径。
#[derive(Debug, Clone)]
pub struct ShareResultDto {
    pub passphrase: String,
    pub record_count: i64,
    pub byte_size: i64,
    pub path: String,
}

/// 时间线导出结果:未加密、可打印的自包含 HTML。与 `ShareResultDto` 不同,
/// 没有口令——导出内容不加密,靠系统「分享」sheet 直接交给医生 / 存下来打印。
#[derive(Debug, Clone)]
pub struct ExportResultDto {
    pub record_count: i64,
    pub byte_size: i64,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct PatientProfileDto {
    pub name: Option<String>,
    pub gender: Option<String>,
    pub birth_date: Option<String>,
    pub age: Option<String>,
    pub record_count: i64,
}
