use crate::{cas, MedmeError, Vault};
use chrono::{DateTime, Utc};
use rusqlite::OptionalExtension;

#[derive(Debug, Clone, PartialEq)]
pub enum DocType {
    LabReport,
    ImagingReport,
    DischargeSummary,
    Prescription,
    ClinicalNote,
    Pathology,
    Surgery,
    Other,
    Unknown,
}
impl DocType {
    pub fn as_str(&self) -> &'static str {
        match self {
            DocType::LabReport => "lab_report",
            DocType::ImagingReport => "imaging_report",
            DocType::DischargeSummary => "discharge_summary",
            DocType::Prescription => "prescription",
            DocType::ClinicalNote => "clinical_note",
            DocType::Pathology => "pathology",
            DocType::Surgery => "surgery",
            DocType::Other => "other",
            DocType::Unknown => "unknown",
        }
    }
    #[allow(clippy::should_implement_trait)] // inherent infallible mapping (Unknown fallback), not std::str::FromStr
    pub fn from_str(s: &str) -> DocType {
        match s {
            "lab_report" => DocType::LabReport,
            "imaging_report" => DocType::ImagingReport,
            "discharge_summary" => DocType::DischargeSummary,
            "prescription" => DocType::Prescription,
            "clinical_note" => DocType::ClinicalNote,
            "pathology" => DocType::Pathology,
            "surgery" => DocType::Surgery,
            "other" => DocType::Other,
            _ => DocType::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum OcrBackendKind {
    Native,
    Onnx,
    Vlm,
    /// Apple Vision (`VNRecognizeTextRequest`) — on-device, offline OCR used on
    /// iOS instead of the oar-ocr model download (impossible in the iOS sandbox).
    AppleVision,
    /// Windows.Media.Ocr (WinRT) — on-device, offline OCR used on the Windows
    /// desktop build as the primary recognizer (PP-OCRv5 ONNX is the fallback).
    WindowsOcr,
    /// Google ML Kit Text Recognition v2 (Chinese, bundled model) — on-device,
    /// offline OCR used on Android instead of the weaker PP-OCRv5 mobile model.
    MlKit,
}
impl OcrBackendKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            OcrBackendKind::Native => "native",
            OcrBackendKind::Onnx => "onnx",
            OcrBackendKind::Vlm => "vlm",
            OcrBackendKind::AppleVision => "apple_vision",
            OcrBackendKind::WindowsOcr => "windows_ocr",
            OcrBackendKind::MlKit => "mlkit",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SourceFile {
    pub id: i64,
    pub content_hash: String,
    pub original_name: String,
    pub mime_type: String,
    pub byte_size: i64,
    pub storage_path: String,
    pub imported_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct Import {
    pub source_file: SourceFile,
    pub deduped: bool,
}

#[derive(Debug, Clone)]
pub struct Document {
    pub id: i64,
    pub source_file_id: i64,
    pub doc_type: DocType,
    pub doc_date: Option<DateTime<Utc>>,
    pub doc_date_end: Option<DateTime<Utc>>,
    pub title: Option<String>,
    pub language: Option<String>,
    pub page_count: i32,
    pub encounter_id: Option<i64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EncounterKind {
    Inpatient,
    Outpatient,
    Emergency,
    Exam,
}
impl EncounterKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EncounterKind::Inpatient => "inpatient",
            EncounterKind::Outpatient => "outpatient",
            EncounterKind::Emergency => "emergency",
            EncounterKind::Exam => "exam",
        }
    }
    #[allow(clippy::should_implement_trait)] // inherent infallible mapping (Outpatient fallback), not std::str::FromStr
    pub fn from_str(s: &str) -> EncounterKind {
        match s {
            "inpatient" => EncounterKind::Inpatient,
            "emergency" => EncounterKind::Emergency,
            "exam" => EncounterKind::Exam,
            _ => EncounterKind::Outpatient,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Encounter {
    pub id: i64,
    pub kind: EncounterKind,
    pub provider: Option<String>,
    pub start_date: Option<DateTime<Utc>>,
    pub end_date: Option<DateTime<Utc>>,
    pub title: Option<String>,
    pub transferred: bool,
    pub created_at: DateTime<Utc>,
}

pub struct NewDocument {
    pub source_file_id: i64,
    pub doc_type: DocType,
    pub doc_date: Option<DateTime<Utc>>,
    pub doc_date_end: Option<DateTime<Utc>>,
    pub title: Option<String>,
    pub language: Option<String>,
    pub page_count: i32,
}

pub struct NewOcr {
    pub document_id: i64,
    pub page_no: i32,
    pub backend: OcrBackendKind,
    pub model_version: String,
    pub text: String,
    pub confidence: Option<f32>,
}

/// One DICOM slice (instance) attached to an imaging-study document.
#[derive(Debug, Clone, PartialEq)]
pub struct ImagingInstance {
    pub id: i64,
    pub document_id: i64,
    pub source_file_id: i64,
    pub series_uid: Option<String>,
    pub series_number: Option<i32>,
    pub instance_number: Option<i32>,
}

/// Request to attach a DICOM slice to a study document (imaging overhaul P1).
pub struct NewImagingInstance {
    /// The study document this slice belongs to (its grouping anchor).
    pub document_id: i64,
    /// The CAS source_file holding this slice's raw `.dcm` bytes.
    pub source_file_id: i64,
    /// StudyInstanceUID — also stamped onto `document.study_uid` on first use.
    pub study_uid: String,
    pub series_uid: Option<String>,
    pub series_number: Option<i32>,
    pub instance_number: Option<i32>,
}

impl Vault {
    pub(crate) fn now_rfc3339() -> String {
        Utc::now().to_rfc3339()
    }

    pub fn import(
        &self,
        original_name: &str,
        mime: &str,
        bytes: &[u8],
    ) -> Result<Import, MedmeError> {
        let hash = cas::sha256_hex(bytes);
        // 已登记?
        if let Some(sf) = self.source_file_by_hash(&hash)? {
            return Ok(Import {
                source_file: sf,
                deduped: true,
            });
        }
        self.store_object(bytes)?;
        let now = Self::now_rfc3339();
        self.append_event(crate::event::Event::FileImported {
            content_hash: hash.clone(),
            original_name: original_name.to_string(),
            mime_type: mime.to_string(),
            byte_size: bytes.len() as i64,
            imported_at: now,
        })?;
        self.materialize()?;
        let sf = self
            .source_file_by_hash(&hash)?
            .ok_or_else(|| MedmeError::Other("insert then missing".into()))?;
        Ok(Import {
            source_file: sf,
            deduped: false,
        })
    }

    fn source_file_by_hash(&self, hash: &str) -> Result<Option<SourceFile>, MedmeError> {
        let row = self.conn().query_row(
            "SELECT id, content_hash, original_name, mime_type, byte_size, storage_path, imported_at
             FROM source_file WHERE content_hash = ?1",
            [hash],
            |r| Ok(SourceFile {
                id: r.get(0)?,
                content_hash: r.get(1)?,
                original_name: r.get(2)?,
                mime_type: r.get(3)?,
                byte_size: r.get(4)?,
                storage_path: r.get(5)?,
                imported_at: parse_dt(r.get::<_, String>(6)?),
            }),
        ).optional()?;
        Ok(row)
    }

    #[cfg(test)]
    pub(crate) fn debug_count(&self, table: &str) -> i64 {
        self.conn()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    pub fn add_document(&self, d: NewDocument) -> Result<Document, MedmeError> {
        let sf = self.source_file_by_id(d.source_file_id)?.ok_or_else(|| {
            MedmeError::Other(format!("source_file {} not found", d.source_file_id))
        })?;
        let now = Self::now_rfc3339();
        let source_file_id = d.source_file_id;
        self.append_event(crate::event::Event::DocumentAdded {
            source_file_hash: sf.content_hash,
            doc_type: d.doc_type.as_str().to_string(),
            doc_date: d.doc_date.map(|x| x.to_rfc3339()),
            doc_date_end: d.doc_date_end.map(|x| x.to_rfc3339()),
            title: d.title,
            language: d.language,
            page_count: d.page_count,
            created_at: now,
        })?;
        self.materialize()?;
        self.document_by_source_file_id(source_file_id)?
            .ok_or_else(|| MedmeError::Other("document missing after materialize".into()))
    }

    pub fn add_ocr(&self, o: NewOcr) -> Result<i64, MedmeError> {
        let doc = self
            .document_by_id(o.document_id)?
            .ok_or_else(|| MedmeError::Other(format!("document {} not found", o.document_id)))?;
        let sf = self.source_file_by_id(doc.source_file_id)?.ok_or_else(|| {
            MedmeError::Other(format!("source_file {} not found", doc.source_file_id))
        })?;
        let (text_hash, _rel, _written) = self.store_object(o.text.as_bytes())?;
        let now = Self::now_rfc3339();
        let (document_id, page_no) = (o.document_id, o.page_no);
        self.append_event(crate::event::Event::OcrAdded {
            document_ref: crate::event::DocRef {
                source_file_hash: sf.content_hash,
            },
            page_no: o.page_no,
            backend: o.backend.as_str().to_string(),
            model_version: o.model_version,
            text_hash,
            confidence: o.confidence,
            created_at: now,
        })?;
        self.materialize()?;
        let ocr_id: i64 = self.conn().query_row(
            "SELECT id FROM ocr_result WHERE document_id = ?1 AND page_no = ?2",
            rusqlite::params![document_id, page_no],
            |r| r.get(0),
        )?;
        Ok(ocr_id)
    }
}

pub(crate) fn parse_dt(s: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&s)
        .map(|d| d.with_timezone(&Utc))
        // 解析失败(如手工改库/损坏行)时回退到 Unix epoch 哨兵值:明显异常、可被发现,
        // 而非用 now() 伪装成真实导入时间。正常路径下本代码写入的都是合法 RFC3339,不会触发。
        .unwrap_or_else(|_| {
            DateTime::from_timestamp(0, 0)
                .expect("Unix epoch (timestamp 0) is always a valid DateTime")
        })
}

#[cfg(test)]
mod tests {
    use crate::Vault;

    #[test]
    fn import_dedups_by_content() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();

        let a = v
            .import("report.pdf", "application/pdf", b"PDFDATA")
            .unwrap();
        assert!(!a.deduped);

        // 同内容、不同文件名 → 命中去重,仍是同一 source_file
        let b = v
            .import("renamed.pdf", "application/pdf", b"PDFDATA")
            .unwrap();
        assert!(b.deduped);
        assert_eq!(a.source_file.id, b.source_file.id);

        let n: i64 = v.debug_count("source_file");
        assert_eq!(n, 1);
    }

    #[test]
    fn parse_dt_valid_and_sentinel() {
        use crate::types::parse_dt;
        let good = parse_dt("2023-05-01T00:00:00+00:00".to_string());
        assert_eq!(good.format("%Y-%m-%d").to_string(), "2023-05-01");
        // 损坏字符串 → epoch 哨兵,不 panic
        let bad = parse_dt("not-a-date".to_string());
        assert_eq!(bad.timestamp(), 0);
    }

    #[test]
    fn add_document_and_ocr_populates_fts() {
        use crate::types::{NewDocument, NewOcr};
        use crate::DocType;
        use crate::OcrBackendKind;

        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        let imp = v.import("r.txt", "text/plain", b"x").unwrap();

        let doc = v
            .add_document(NewDocument {
                source_file_id: imp.source_file.id,
                doc_type: DocType::LabReport,
                doc_date: None,
                doc_date_end: None,
                title: Some("血常规".into()),
                language: Some("zh".into()),
                page_count: 1,
            })
            .unwrap();
        assert!(doc.id > 0);

        let ocr_id = v
            .add_ocr(NewOcr {
                document_id: doc.id,
                page_no: 1,
                backend: OcrBackendKind::Native,
                model_version: "text-layer".into(),
                text: "肌酐 Creatinine 120 umol/L".into(),
                confidence: None,
            })
            .unwrap();
        assert!(ocr_id > 0);
        assert_eq!(v.debug_count("document_fts"), 1);

        // body 应为分词后文本,含中英 token
        let body: String = v
            .conn()
            .query_row(
                "SELECT body FROM document_fts WHERE document_id = ?1",
                [doc.id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(body.contains("肌酐"));
        assert!(body.contains("Creatinine"));
        assert!(body.split_whitespace().count() >= 3); // 已分词
    }
}
