//! Replay the append-only event log into the derived SQLite tables.
//!
//! `medme.db` is a cache: `materialize` applies events past the `applied_seq`
//! watermark (stored in the `meta` table), and `rebuild_from_log` wipes the
//! derived tables and replays the whole log from scratch. Both are idempotent.

use crate::event::{DocRef, Event, LogEntry};
use crate::{cas, MedmeError, Vault};
use rusqlite::{OptionalExtension, Transaction};

impl Vault {
    pub(crate) fn get_meta(&self, key: &str) -> Result<Option<String>, MedmeError> {
        Ok(self
            .conn()
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub(crate) fn set_meta(&self, key: &str, value: &str) -> Result<(), MedmeError> {
        self.conn().execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

    fn set_applied_seq_tx(tx: &Transaction, seq: i64) -> Result<(), MedmeError> {
        tx.execute(
            "INSERT INTO meta(key, value) VALUES ('applied_seq', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [seq.to_string()],
        )?;
        Ok(())
    }

    pub(crate) fn ensure_device_id(&self) -> Result<String, MedmeError> {
        if let Some(id) = self.get_meta("device_id")? {
            return Ok(id);
        }
        let id = generate_device_id();
        self.set_meta("device_id", &id)?;
        Ok(id)
    }

    pub(crate) fn applied_seq(&self) -> Result<i64, MedmeError> {
        Ok(self
            .get_meta("applied_seq")?
            .and_then(|s| s.parse().ok())
            .unwrap_or(0))
    }

    /// Apply any log events past the watermark to the DB, then advance the
    /// watermark. Idempotent: a no-op if nothing is pending.
    pub fn materialize(&self) -> Result<(), MedmeError> {
        let applied = self.applied_seq()?;
        let entries = self.log.read_all()?;
        let pending: Vec<&LogEntry> = entries.iter().filter(|e| e.seq > applied).collect();
        if pending.is_empty() {
            return Ok(());
        }
        let tx = self.conn().unchecked_transaction()?;
        let mut max_seq = applied;
        for entry in &pending {
            apply_event(&tx, self, &entry.event)?;
            if entry.seq > max_seq {
                max_seq = entry.seq;
            }
        }
        Self::set_applied_seq_tx(&tx, max_seq)?;
        tx.commit()?;
        Ok(())
    }

    /// Clear the derived tables and replay the whole log from scratch. The
    /// key rebuildability property: `medme.db` can be deleted (or its
    /// derived tables wiped) and reconstructed byte-for-byte-equivalent
    /// content from `objects/` + `log/` alone.
    pub fn rebuild_from_log(&self) -> Result<(), MedmeError> {
        {
            let tx = self.conn().unchecked_transaction()?;
            tx.execute("DELETE FROM document_fts", [])?;
            tx.execute("DELETE FROM ocr_result", [])?;
            tx.execute("DELETE FROM document", [])?;
            tx.execute("DELETE FROM encounter", [])?;
            tx.execute("DELETE FROM source_file", [])?;
            Self::set_applied_seq_tx(&tx, 0)?;
            tx.commit()?;
        }
        self.materialize()?;
        // encounters are pure derived-of-derived (never logged) — recompute after replay
        self.rebuild_encounters()?;
        Ok(())
    }

    /// One-time migration for a pre-refactor, DB-only vault: synthesize
    /// `FileImported` / `DocumentAdded` / `OcrAdded` events from the current
    /// DB rows (storing each OCR text into CAS to get its hash), then mark
    /// the watermark as fully applied since the DB already reflects them.
    pub(crate) fn migrate_db_to_log(&self) -> Result<(), MedmeError> {
        struct Sf {
            content_hash: String,
            original_name: String,
            mime_type: String,
            byte_size: i64,
            imported_at: String,
        }
        let sfs: Vec<Sf> = {
            let mut stmt = self.conn().prepare(
                "SELECT content_hash, original_name, mime_type, byte_size, imported_at
                 FROM source_file ORDER BY id ASC",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(Sf {
                    content_hash: r.get(0)?,
                    original_name: r.get(1)?,
                    mime_type: r.get(2)?,
                    byte_size: r.get(3)?,
                    imported_at: r.get(4)?,
                })
            })?;
            rows.collect::<Result<_, _>>()?
        };
        for sf in sfs {
            self.append_event(Event::FileImported {
                content_hash: sf.content_hash,
                original_name: sf.original_name,
                mime_type: sf.mime_type,
                byte_size: sf.byte_size,
                imported_at: sf.imported_at,
            })?;
        }

        struct Doc {
            source_file_hash: String,
            doc_type: String,
            doc_date: Option<String>,
            doc_date_end: Option<String>,
            title: Option<String>,
            language: Option<String>,
            page_count: i32,
            created_at: String,
        }
        let docs: Vec<Doc> = {
            let mut stmt = self.conn().prepare(
                "SELECT sf.content_hash, d.doc_type, d.doc_date, d.doc_date_end, d.title,
                        d.language, d.page_count, d.created_at
                 FROM document d JOIN source_file sf ON d.source_file_id = sf.id
                 ORDER BY d.id ASC",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(Doc {
                    source_file_hash: r.get(0)?,
                    doc_type: r.get(1)?,
                    doc_date: r.get(2)?,
                    doc_date_end: r.get(3)?,
                    title: r.get(4)?,
                    language: r.get(5)?,
                    page_count: r.get(6)?,
                    created_at: r.get(7)?,
                })
            })?;
            rows.collect::<Result<_, _>>()?
        };
        for d in docs {
            self.append_event(Event::DocumentAdded {
                source_file_hash: d.source_file_hash,
                doc_type: d.doc_type,
                doc_date: d.doc_date,
                doc_date_end: d.doc_date_end,
                title: d.title,
                language: d.language,
                page_count: d.page_count,
                created_at: d.created_at,
            })?;
        }

        struct Ocr {
            source_file_hash: String,
            page_no: i32,
            backend: String,
            model_version: String,
            text: String,
            confidence: Option<f32>,
            created_at: String,
        }
        let ocrs: Vec<Ocr> = {
            let mut stmt = self.conn().prepare(
                "SELECT sf.content_hash, o.page_no, o.backend, o.model_version, o.text,
                        o.confidence, o.created_at
                 FROM ocr_result o
                 JOIN document d ON o.document_id = d.id
                 JOIN source_file sf ON d.source_file_id = sf.id
                 ORDER BY o.id ASC",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(Ocr {
                    source_file_hash: r.get(0)?,
                    page_no: r.get(1)?,
                    backend: r.get(2)?,
                    model_version: r.get(3)?,
                    text: r.get(4)?,
                    confidence: r.get(5)?,
                    created_at: r.get(6)?,
                })
            })?;
            rows.collect::<Result<_, _>>()?
        };
        for o in ocrs {
            let (text_hash, _rel, _written) = self.store_object(o.text.as_bytes())?;
            self.append_event(Event::OcrAdded {
                document_ref: DocRef {
                    source_file_hash: o.source_file_hash,
                },
                page_no: o.page_no,
                backend: o.backend,
                model_version: o.model_version,
                text_hash,
                confidence: o.confidence,
                created_at: o.created_at,
            })?;
        }

        let max_seq = self.log.max_seq()?;
        if max_seq > 0 {
            let tx = self.conn().unchecked_transaction()?;
            Self::set_applied_seq_tx(&tx, max_seq)?;
            tx.commit()?;
        }
        Ok(())
    }
}

fn generate_device_id() -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    h.update(nanos.to_le_bytes());
    h.update(std::process::id().to_le_bytes());
    format!("{:x}", h.finalize())
}

fn apply_event(tx: &Transaction, vault: &Vault, event: &Event) -> Result<(), MedmeError> {
    match event {
        Event::FileImported {
            content_hash,
            original_name,
            mime_type,
            byte_size,
            imported_at,
        } => {
            let relpath = cas::object_relpath(content_hash);
            tx.execute(
                "INSERT INTO source_file
                 (content_hash, original_name, mime_type, byte_size, storage_path, imported_at)
                 VALUES (?1,?2,?3,?4,?5,?6)",
                rusqlite::params![content_hash, original_name, mime_type, byte_size, relpath, imported_at],
            )?;
        }
        Event::DocumentAdded {
            source_file_hash,
            doc_type,
            doc_date,
            doc_date_end,
            title,
            language,
            page_count,
            created_at,
        } => {
            let source_file_id: i64 = tx.query_row(
                "SELECT id FROM source_file WHERE content_hash = ?1",
                [source_file_hash],
                |r| r.get(0),
            )?;
            tx.execute(
                "INSERT INTO document
                 (source_file_id, doc_type, doc_date, doc_date_end, title, language, page_count, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                rusqlite::params![
                    source_file_id, doc_type, doc_date, doc_date_end, title, language, page_count, created_at
                ],
            )?;
        }
        Event::OcrAdded {
            document_ref,
            page_no,
            backend,
            model_version,
            text_hash,
            confidence,
            created_at,
        } => {
            let document_id: i64 = tx.query_row(
                "SELECT d.id FROM document d JOIN source_file sf ON d.source_file_id = sf.id
                 WHERE sf.content_hash = ?1",
                [&document_ref.source_file_hash],
                |r| r.get(0),
            )?;
            let relpath = cas::object_relpath(text_hash);
            let text = std::fs::read_to_string(vault.root_join(&relpath))?;
            tx.execute(
                "INSERT INTO ocr_result
                 (document_id, page_no, backend, model_version, text, confidence, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
                rusqlite::params![document_id, page_no, backend, model_version, text, confidence, created_at],
            )?;
            let title: Option<String> = tx.query_row(
                "SELECT title FROM document WHERE id = ?1",
                [document_id],
                |r| r.get(0),
            )?;
            let body = crate::tokenize::tokenize(&text);
            let title_tok = title.as_deref().map(crate::tokenize::tokenize);
            tx.execute(
                "INSERT INTO document_fts(document_id, title, body) VALUES (?1,?2,?3)",
                rusqlite::params![document_id, title_tok, body],
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{NewDocument, NewOcr};
    use crate::{DocType, OcrBackendKind};

    #[test]
    fn write_appends_event_and_materializes() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();

        let imp = v.import("a.txt", "text/plain", b"hello world").unwrap();
        let doc = v
            .add_document(NewDocument {
                source_file_id: imp.source_file.id,
                doc_type: DocType::LabReport,
                doc_date: None,
                doc_date_end: None,
                title: Some("t".into()),
                language: None,
                page_count: 1,
            })
            .unwrap();
        v.add_ocr(NewOcr {
            document_id: doc.id,
            page_no: 1,
            backend: OcrBackendKind::Native,
            model_version: "text-layer".into(),
            text: "some ocr text".into(),
            confidence: None,
        })
        .unwrap();

        assert_eq!(v.debug_count("source_file"), 1);
        assert_eq!(v.debug_count("document"), 1);
        assert_eq!(v.debug_count("ocr_result"), 1);

        let events = v.log.read_all().unwrap();
        assert_eq!(events.len(), 3, "one event per write op");
        assert!(matches!(events[0].event, Event::FileImported { .. }));
        assert!(matches!(events[1].event, Event::DocumentAdded { .. }));
        assert!(matches!(events[2].event, Event::OcrAdded { .. }));
    }

    #[test]
    fn db_is_rebuildable_from_log() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();

        let imp = v.import("a.txt", "text/plain", b"hello world").unwrap();
        let doc = v
            .add_document(NewDocument {
                source_file_id: imp.source_file.id,
                doc_type: DocType::LabReport,
                doc_date: Some(chrono::Utc::now()),
                doc_date_end: None,
                title: Some("血常规".into()),
                language: Some("zh".into()),
                page_count: 1,
            })
            .unwrap();
        v.add_ocr(NewOcr {
            document_id: doc.id,
            page_no: 1,
            backend: OcrBackendKind::Native,
            model_version: "text-layer".into(),
            text: "肌酐 Creatinine 120".into(),
            confidence: None,
        })
        .unwrap();
        v.rebuild_encounters().unwrap();

        let before_timeline = v.timeline().unwrap();
        let before_text = v.ocr_text(doc.id).unwrap();
        let before_search = v.search("Creatinine", 10).unwrap().len();
        let before_sf_count = v.debug_count("source_file");
        let before_encounter_count = v.debug_count("encounter");

        v.rebuild_from_log().unwrap();

        let after_timeline = v.timeline().unwrap();
        assert_eq!(before_timeline.len(), after_timeline.len());
        assert_eq!(before_timeline[0].title, after_timeline[0].title);
        assert_eq!(v.ocr_text(doc.id).unwrap(), before_text);
        assert_eq!(v.search("Creatinine", 10).unwrap().len(), before_search);
        assert_eq!(v.debug_count("source_file"), before_sf_count);
        assert_eq!(v.debug_count("encounter"), before_encounter_count);
    }

    #[test]
    fn migrate_db_only_vault_creates_log() {
        let dir = tempfile::tempdir().unwrap();
        {
            let v = Vault::open(dir.path()).unwrap();
            let imp = v.import("a.txt", "text/plain", b"legacy data").unwrap();
            v.add_document(NewDocument {
                source_file_id: imp.source_file.id,
                doc_type: DocType::LabReport,
                doc_date: None,
                doc_date_end: None,
                title: Some("old doc".into()),
                language: None,
                page_count: 1,
            })
            .unwrap();
        } // drop: close the sqlite connection before poking the file directly

        // Simulate a pre-refactor, DB-only vault: drop the log + reset the watermark.
        std::fs::remove_dir_all(dir.path().join("log")).unwrap();
        {
            let conn = rusqlite::Connection::open(dir.path().join("medme.db")).unwrap();
            conn.execute("UPDATE meta SET value = '0' WHERE key = 'applied_seq'", [])
                .unwrap();
        }

        let v2 = Vault::open(dir.path()).unwrap();
        let events = v2.log.read_all().unwrap();
        assert_eq!(events.len(), 2, "FileImported + DocumentAdded regenerated");
        assert_eq!(v2.debug_count("document"), 1);
        assert_eq!(v2.debug_count("source_file"), 1);
        assert_eq!(
            v2.applied_seq().unwrap(),
            events.iter().map(|e| e.seq).max().unwrap(),
            "watermark marked fully-applied since the DB already reflected these rows"
        );
    }
}
