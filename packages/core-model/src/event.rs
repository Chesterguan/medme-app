//! Event types for the append-only log (see `docs/011_Storage_Sync.md`).
//!
//! One variant per existing `Vault` write op, kept granular so replaying the
//! log reproduces exactly what the write methods did. Events reference other
//! rows by content hash (never by DB autoincrement id) so they stay valid
//! independent of any particular SQLite database.

use crate::MedmeError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Stable reference to a document via its source file's content hash.
/// v0.1 has one document per source file (`UNIQUE(source_file_id)`), so the
/// source file's hash is a sufficient, DB-independent document key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocRef {
    pub source_file_hash: String,
}

/// One granular write operation, immutable once appended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Event {
    FileImported {
        content_hash: String,
        original_name: String,
        mime_type: String,
        byte_size: i64,
        imported_at: String,
    },
    DocumentAdded {
        source_file_hash: String,
        doc_type: String,
        doc_date: Option<String>,
        doc_date_end: Option<String>,
        title: Option<String>,
        language: Option<String>,
        page_count: i32,
        created_at: String,
    },
    OcrAdded {
        document_ref: DocRef,
        page_no: i32,
        backend: String,
        model_version: String,
        text_hash: String,
        confidence: Option<f32>,
        created_at: String,
    },
}

/// One line in the append-only log: an `Event` plus the envelope needed for
/// ordering, dedup, and (future) sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    /// sha256 of the canonical JSON of `event` alone (not this envelope) —
    /// the same logical write appended on two devices collapses to one id.
    pub event_id: String,
    pub seq: i64,
    pub ts: String,
    pub device_id: String,
    #[serde(flatten)]
    pub event: Event,
}

impl LogEntry {
    pub fn new(seq: i64, ts: String, device_id: String, event: Event) -> Result<Self, MedmeError> {
        Ok(LogEntry {
            event_id: event_id(&event)?,
            seq,
            ts,
            device_id,
            event,
        })
    }
}

fn event_id(event: &Event) -> Result<String, MedmeError> {
    // serde_json serializes struct/enum fields in declaration order (not
    // sorted), so this is deterministic given the fixed definitions above —
    // sufficient "canonical JSON" for a single-implementation content id.
    let bytes = serde_json::to_vec(event)?;
    let mut h = Sha256::new();
    h.update(&bytes);
    Ok(format!("{:x}", h.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn imported(byte_size: i64) -> Event {
        Event::FileImported {
            content_hash: "abc".into(),
            original_name: "a.pdf".into(),
            mime_type: "application/pdf".into(),
            byte_size,
            imported_at: "2024-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn event_id_is_deterministic_and_content_addressed() {
        assert_eq!(event_id(&imported(3)).unwrap(), event_id(&imported(3)).unwrap());
        assert_ne!(event_id(&imported(3)).unwrap(), event_id(&imported(4)).unwrap());
    }

    #[test]
    fn log_entry_round_trips_through_json() {
        let entry = LogEntry::new(1, "2024-01-01T00:00:00Z".into(), "dev1".into(), imported(3)).unwrap();
        let line = serde_json::to_string(&entry).unwrap();
        let back: LogEntry = serde_json::from_str(&line).unwrap();
        assert_eq!(back.event_id, entry.event_id);
        assert_eq!(back.seq, entry.seq);
        assert_eq!(back.event, entry.event);
    }
}
