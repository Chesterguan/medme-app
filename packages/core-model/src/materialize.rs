//! Replay the append-only event log into the derived SQLite tables.
//!
//! `medme.db` is a cache: `materialize` applies events past the `applied_seq`
//! watermark (stored in the `meta` table), and `rebuild_from_log` wipes the
//! derived tables and replays the whole log from scratch. Both are idempotent.

use crate::event::{DocRef, Event, LogEntry};
use crate::{cas, MedmeError, Vault};
use rusqlite::{OptionalExtension, Transaction};
use std::collections::{HashMap, HashSet};

/// Outcome of applying a single event to the derived DB.
enum ApplyOutcome {
    /// Event was reflected in the DB (or was already present — idempotent).
    Applied,
    /// Could not be applied *yet* because a CAS object it needs hasn't synced
    /// in. Left unapplied so a later `materialize` retries it once the blob
    /// arrives — never propagated as an error (must not block `Vault::open`).
    Deferred,
}

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

    /// Per-device high-water map `{device_id: max_applied_seq}`, stored as JSON
    /// under the `applied_seq_map` meta key. Absent → empty map (first run after
    /// this change re-applies everything, harmless because projection is
    /// idempotent). Replaces the old global scalar `applied_seq`, which dropped
    /// a synced peer's events whose per-device seq was ≤ the local scalar.
    pub(crate) fn applied_seq_map(&self) -> Result<HashMap<String, i64>, MedmeError> {
        match self.get_meta("applied_seq_map")? {
            Some(json) => Ok(serde_json::from_str(&json)?),
            None => Ok(HashMap::new()),
        }
    }

    fn set_applied_seq_map_tx(
        tx: &Transaction,
        map: &HashMap<String, i64>,
    ) -> Result<(), MedmeError> {
        let json = serde_json::to_string(map)?;
        tx.execute(
            "INSERT INTO meta(key, value) VALUES ('applied_seq_map', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [json],
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

    /// Backwards-compatible scalar accessor: the max watermark across all
    /// devices. (The real gate is now the per-device `applied_seq_map`.)
    #[cfg(test)]
    pub(crate) fn applied_seq(&self) -> Result<i64, MedmeError> {
        Ok(self.applied_seq_map()?.values().copied().max().unwrap_or(0))
    }

    /// Apply any log events past each device's watermark to the DB, then
    /// advance that device's watermark. Idempotent: a no-op if nothing is
    /// pending. Because `seq` is per-device (two devices can reuse the same
    /// value), the gate is a per-device high-water map, not a global scalar.
    pub fn materialize(&self) -> Result<(), MedmeError> {
        let map = self.applied_seq_map()?;
        let entries = self.log.read_all()?;
        let pending: Vec<&LogEntry> = entries
            .iter()
            .filter(|e| e.seq > map.get(&e.device_id).copied().unwrap_or(0))
            .collect();
        if pending.is_empty() {
            return Ok(());
        }
        let tx = self.conn().unchecked_transaction()?;
        let mut new_map = map;
        // Devices with a deferred (not-yet-syncable) event: stop advancing their
        // watermark and skip their remaining events so we never jump the gap.
        let mut stopped: HashSet<String> = HashSet::new();
        for entry in &pending {
            if stopped.contains(&entry.device_id) {
                continue;
            }
            match apply_event(&tx, self, &entry.event)? {
                ApplyOutcome::Applied => {
                    let cur = new_map.entry(entry.device_id.clone()).or_insert(0);
                    if entry.seq > *cur {
                        *cur = entry.seq;
                    }
                }
                ApplyOutcome::Deferred => {
                    stopped.insert(entry.device_id.clone());
                }
            }
        }
        Self::set_applied_seq_map_tx(&tx, &new_map)?;
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
            tx.execute("DELETE FROM imaging_instance", [])?;
            tx.execute("DELETE FROM document", [])?;
            tx.execute("DELETE FROM encounter", [])?;
            tx.execute("DELETE FROM source_file", [])?;
            Self::set_applied_seq_map_tx(&tx, &HashMap::new())?;
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

        // Mark the DB as fully applied by seeding each device's watermark with
        // its own max seq from the log (the DB already reflects these rows).
        let mut map: HashMap<String, i64> = HashMap::new();
        for e in self.log.read_all()? {
            let cur = map.entry(e.device_id.clone()).or_insert(0);
            if e.seq > *cur {
                *cur = e.seq;
            }
        }
        if !map.is_empty() {
            let tx = self.conn().unchecked_transaction()?;
            Self::set_applied_seq_map_tx(&tx, &map)?;
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

fn apply_event(
    tx: &Transaction,
    vault: &Vault,
    event: &Event,
) -> Result<ApplyOutcome, MedmeError> {
    match event {
        Event::FileImported {
            content_hash,
            original_name,
            mime_type,
            byte_size,
            imported_at,
        } => {
            let relpath = cas::object_relpath(content_hash);
            // Idempotent on the natural key UNIQUE(content_hash): the same file
            // imported on two devices collapses to one row instead of aborting.
            tx.execute(
                "INSERT INTO source_file
                 (content_hash, original_name, mime_type, byte_size, storage_path, imported_at)
                 VALUES (?1,?2,?3,?4,?5,?6)
                 ON CONFLICT(content_hash) DO NOTHING",
                rusqlite::params![
                    content_hash,
                    original_name,
                    mime_type,
                    byte_size,
                    relpath,
                    imported_at
                ],
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
            // Idempotent on the natural key UNIQUE(source_file_id).
            tx.execute(
                "INSERT INTO document
                 (source_file_id, doc_type, doc_date, doc_date_end, title, language, page_count, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(source_file_id) DO NOTHING",
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
            // Idempotency guard on UNIQUE(document_id, page_no): if the OCR row
            // already exists (re-applied event / same page from two devices),
            // skip the CAS read AND the FTS insert entirely — cheap re-apply,
            // no duplicate FTS row.
            let already: Option<i64> = tx
                .query_row(
                    "SELECT id FROM ocr_result WHERE document_id = ?1 AND page_no = ?2",
                    rusqlite::params![document_id, page_no],
                    |r| r.get(0),
                )
                .optional()?;
            if already.is_some() {
                return Ok(ApplyOutcome::Applied);
            }
            let relpath = cas::object_relpath(text_hash);
            // The CAS object may not have synced in yet (log arrives before the
            // blob). Treat a missing object as Deferred — retried once it lands
            // — rather than aborting the whole materialize / `Vault::open`.
            let text = match std::fs::read_to_string(vault.root_join(&relpath)) {
                Ok(t) => t,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(ApplyOutcome::Deferred);
                }
                Err(e) => return Err(e.into()),
            };
            tx.execute(
                "INSERT INTO ocr_result
                 (document_id, page_no, backend, model_version, text, confidence, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
                rusqlite::params![
                    document_id,
                    page_no,
                    backend,
                    model_version,
                    text,
                    confidence,
                    created_at
                ],
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
        // 影像切片挂载(imaging overhaul P1):把 DICOM 切片行插入 imaging_instance,
        // 并把 study_uid 落到 study 文档上(供 study→document 查找)。两个引用都用
        // 内容哈希解析成当前库的行 id,保证 rebuild_from_log 脱库重放也一致。
        Event::ImagingInstanceAdded {
            document_ref,
            source_file_hash,
            study_uid,
            series_uid,
            series_number,
            instance_number,
            created_at: _,
        } => {
            let document_id: i64 = tx.query_row(
                "SELECT d.id FROM document d JOIN source_file sf ON d.source_file_id = sf.id
                 WHERE sf.content_hash = ?1",
                [&document_ref.source_file_hash],
                |r| r.get(0),
            )?;
            let source_file_id: i64 = tx.query_row(
                "SELECT id FROM source_file WHERE content_hash = ?1",
                [source_file_hash],
                |r| r.get(0),
            )?;
            // Idempotent on the natural key (document_id, source_file_id): one
            // slice attached once per study, even if it arrives from two devices.
            tx.execute(
                "INSERT INTO imaging_instance
                 (document_id, source_file_id, series_uid, series_number, instance_number)
                 VALUES (?1,?2,?3,?4,?5)
                 ON CONFLICT(document_id, source_file_id) DO NOTHING",
                rusqlite::params![
                    document_id,
                    source_file_id,
                    series_uid,
                    series_number,
                    instance_number
                ],
            )?;
            // Stamp study_uid on the document (first instance wins; idempotent).
            tx.execute(
                "UPDATE document SET study_uid = ?1 WHERE id = ?2 AND study_uid IS NULL",
                rusqlite::params![study_uid, document_id],
            )?;
        }
        // 审计事件:纯粹的日志留痕(见 crate::audit),对 DB 投影是 no-op —— 不
        // 建任何表行,`rebuild_from_log` 重放时必须能安全跳过而不报错。
        Event::ExportPerformed { .. } | Event::ShareCreated { .. } => {}
    }
    Ok(ApplyOutcome::Applied)
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

    // ---- per-device log segmentation (docs/013 §3, §6) ----------------------

    /// Recursively copy `src` dir contents into `dst` (used to merge a second
    /// device's CAS objects into a shared vault in the tests below).
    fn copy_dir_into(src: &std::path::Path, dst: &std::path::Path) {
        std::fs::create_dir_all(dst).unwrap();
        for e in std::fs::read_dir(src).unwrap() {
            let e = e.unwrap();
            let to = dst.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                copy_dir_into(&e.path(), &to);
            } else {
                std::fs::copy(e.path(), &to).unwrap();
            }
        }
    }

    fn only_segment(log_dir: &std::path::Path) -> std::path::PathBuf {
        std::fs::read_dir(log_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.extension().and_then(|x| x.to_str()) == Some("jsonl"))
            .expect("one jsonl segment")
    }

    /// Seed a fresh vault at `dir` with one imported+OCR'd doc whose OCR body
    /// contains `needle` (searchable). Returns nothing; caller inspects `dir`.
    fn seed_doc(dir: &std::path::Path, name: &str, needle: &str) {
        let v = Vault::open(dir).unwrap();
        let imp = v.import(name, "text/plain", needle.as_bytes()).unwrap();
        let doc = v
            .add_document(NewDocument {
                source_file_id: imp.source_file.id,
                doc_type: DocType::LabReport,
                doc_date: Some(chrono::Utc::now()),
                doc_date_end: None,
                title: Some(name.into()),
                language: None,
                page_count: 1,
            })
            .unwrap();
        v.add_ocr(NewOcr {
            document_id: doc.id,
            page_no: 1,
            backend: OcrBackendKind::Native,
            model_version: "text-layer".into(),
            text: needle.into(),
            confidence: None,
        })
        .unwrap();
    }

    #[test]
    fn new_events_land_in_per_device_segment() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        v.import("a.txt", "text/plain", b"hello").unwrap();

        let seg = only_segment(&dir.path().join("log"));
        let fname = seg.file_name().unwrap().to_str().unwrap();
        assert!(
            fname.starts_with(&v.device_id) && fname.ends_with(".jsonl"),
            "segment {fname} must be namespaced by this device_id {}",
            v.device_id
        );
        assert!(
            !dir.path().join("log/000001.jsonl").exists(),
            "new writes must not go to a legacy shared segment"
        );
    }

    #[test]
    fn merges_legacy_log_and_second_device_segment_on_rebuild() {
        // Device A: seed a doc, then rename its segment to the pre-refactor
        // single-log name `000001.jsonl` to simulate an existing vault.
        let a = tempfile::tempdir().unwrap();
        seed_doc(a.path(), "alpha.txt", "AlphaUniqueNeedle");
        let a_seg = only_segment(&a.path().join("log"));
        std::fs::rename(&a_seg, a.path().join("log/000001.jsonl")).unwrap();

        // Device B: seed a *different* doc in its own vault, then splice its
        // segment + CAS objects into device A's vault as a second device.
        let b = tempfile::tempdir().unwrap();
        seed_doc(b.path(), "beta.txt", "BetaUniqueNeedle");
        let b_seg = only_segment(&b.path().join("log"));
        std::fs::copy(&b_seg, a.path().join("log/otherdevice-000001.jsonl")).unwrap();
        copy_dir_into(&b.path().join("objects"), &a.path().join("objects"));

        // Wipe the derived cache so the state is rebuilt purely from the merged
        // segments + CAS, exactly as a fresh device syncing the folder would.
        std::fs::remove_file(a.path().join("medme.db")).unwrap();

        let v = Vault::open(a.path()).unwrap();
        v.rebuild_from_log().unwrap();

        assert_eq!(
            v.debug_count("source_file"),
            2,
            "both devices' files present"
        );
        assert_eq!(v.debug_count("document"), 2, "both devices' docs present");
        assert_eq!(v.search("AlphaUniqueNeedle", 10).unwrap().len(), 1);
        assert_eq!(v.search("BetaUniqueNeedle", 10).unwrap().len(), 1);
    }

    #[test]
    fn rebuild_is_deterministic_across_repeated_runs() {
        // A two-device vault (legacy A + device B), same construction as above.
        let a = tempfile::tempdir().unwrap();
        seed_doc(a.path(), "alpha.txt", "AlphaUniqueNeedle");
        let a_seg = only_segment(&a.path().join("log"));
        std::fs::rename(&a_seg, a.path().join("log/000001.jsonl")).unwrap();
        let b = tempfile::tempdir().unwrap();
        seed_doc(b.path(), "beta.txt", "BetaUniqueNeedle");
        let b_seg = only_segment(&b.path().join("log"));
        std::fs::copy(&b_seg, a.path().join("log/otherdevice-000001.jsonl")).unwrap();
        copy_dir_into(&b.path().join("objects"), &a.path().join("objects"));
        std::fs::remove_file(a.path().join("medme.db")).unwrap();

        let v = Vault::open(a.path()).unwrap();
        v.rebuild_from_log().unwrap();
        let snap = |v: &Vault| {
            (
                v.debug_count("source_file"),
                v.debug_count("document"),
                v.debug_count("ocr_result"),
                v.timeline()
                    .unwrap()
                    .iter()
                    .map(|t| t.title.clone())
                    .collect::<Vec<_>>(),
            )
        };
        let first = snap(&v);
        // Rebuilding again must land on byte-identical derived state.
        v.rebuild_from_log().unwrap();
        assert_eq!(first, snap(&v), "rebuild must be deterministic");
    }

    // ---- multi-device data-integrity fixes (A/B/C) --------------------------

    /// Fix A: two devices import the SAME bytes (→ identical content_hash) with
    /// different device_id/ts. Splicing both segments + the shared CAS and
    /// `rebuild_from_log` must SUCCEED and collapse to exactly one source_file
    /// (idempotent projection), not abort on the UNIQUE(content_hash) collision.
    #[test]
    fn duplicate_content_across_devices_rebuilds() {
        let a = tempfile::tempdir().unwrap();
        seed_doc(a.path(), "dup.txt", "SharedDupNeedle");
        let a_seg = only_segment(&a.path().join("log"));
        std::fs::rename(&a_seg, a.path().join("log/000001.jsonl")).unwrap();

        // A *different* device imports the identical bytes → same content_hash,
        // same text_hash, but its own device_id inside the envelope.
        let b = tempfile::tempdir().unwrap();
        seed_doc(b.path(), "dup.txt", "SharedDupNeedle");
        let b_seg = only_segment(&b.path().join("log"));
        std::fs::copy(&b_seg, a.path().join("log/peerdevice-000001.jsonl")).unwrap();
        copy_dir_into(&b.path().join("objects"), &a.path().join("objects"));

        std::fs::remove_file(a.path().join("medme.db")).unwrap();

        let v = Vault::open(a.path()).unwrap();
        // Must not error on the duplicate-content collision.
        v.rebuild_from_log().unwrap();

        assert_eq!(
            v.debug_count("source_file"),
            1,
            "identical content from two devices dedups to one source_file"
        );
        assert_eq!(v.debug_count("document"), 1, "one document");
        assert_eq!(v.debug_count("ocr_result"), 1, "one OCR row");
        assert_eq!(v.search("SharedDupNeedle", 10).unwrap().len(), 1);
    }

    /// Fix B: device A materializes its own events (advancing A's watermark),
    /// then a synced peer's segment is spliced in whose event seqs are ≤ A's
    /// watermark. A plain `materialize()` (NOT a rebuild) must still apply the
    /// peer's events — a single global scalar watermark silently drops them.
    #[test]
    fn synced_peer_event_with_colliding_seq_is_applied() {
        let a = tempfile::tempdir().unwrap();
        seed_doc(a.path(), "alpha.txt", "AlphaWatermarkNeedle");
        // A's per-device watermark is now at its max seq (e.g. 3).

        // Peer B seeds its own doc; its event seqs (1..=3) collide with A's.
        let b = tempfile::tempdir().unwrap();
        seed_doc(b.path(), "beta.txt", "BetaWatermarkNeedle");
        let b_seg = only_segment(&b.path().join("log"));
        std::fs::copy(&b_seg, a.path().join("log/peerdevice-000001.jsonl")).unwrap();
        copy_dir_into(&b.path().join("objects"), &a.path().join("objects"));

        // Reopen (keeping the derived DB, so this is the incremental
        // materialize path, not a rebuild) and materialize.
        let v = Vault::open(a.path()).unwrap();
        v.materialize().unwrap();

        assert_eq!(
            v.search("BetaWatermarkNeedle", 10).unwrap().len(),
            1,
            "peer's low-seq event must be applied despite colliding with A's watermark"
        );
        assert_eq!(v.debug_count("document"), 2, "both devices' docs present");
    }

    /// Fix C: a log references an OcrAdded whose CAS text object hasn't synced
    /// yet. `Vault::open`/`materialize` must NOT error (the missing object is
    /// deferred, other events still apply); once the object arrives, a later
    /// `materialize()` applies the deferred OCR.
    #[test]
    fn missing_cas_object_does_not_block_open_and_applies_later() {
        let dir = tempfile::tempdir().unwrap();
        seed_doc(dir.path(), "miss.txt", "MissingObjNeedle");

        // Remove the OCR text object from CAS (simulating a not-yet-synced blob)
        // and wipe the derived cache so open must re-materialize from log + CAS.
        let text_hash = cas::sha256_hex("MissingObjNeedle".as_bytes());
        let obj = dir.path().join(cas::object_relpath(&text_hash));
        let saved = std::fs::read(&obj).unwrap();
        std::fs::remove_file(&obj).unwrap();
        std::fs::remove_file(dir.path().join("medme.db")).unwrap();

        // Must not error even though the OCR object is absent.
        let v = Vault::open(dir.path()).unwrap();
        assert_eq!(v.debug_count("source_file"), 1, "FileImported still applied");
        assert_eq!(v.debug_count("document"), 1, "DocumentAdded still applied");
        assert_eq!(
            v.debug_count("ocr_result"),
            0,
            "OCR deferred while its object is missing"
        );

        // The blob arrives; re-materialize → the deferred OCR now applies.
        std::fs::write(&obj, &saved).unwrap();
        v.materialize().unwrap();
        assert_eq!(v.debug_count("ocr_result"), 1, "deferred OCR applied once blob present");
        assert_eq!(v.search("MissingObjNeedle", 10).unwrap().len(), 1);
    }

    #[test]
    fn round_trip_import_many_then_rebuild_matches() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        for i in 0..5 {
            let imp = v
                .import(
                    &format!("doc{i}.txt"),
                    "text/plain",
                    format!("body {i}").as_bytes(),
                )
                .unwrap();
            let doc = v
                .add_document(NewDocument {
                    source_file_id: imp.source_file.id,
                    doc_type: DocType::LabReport,
                    doc_date: Some(chrono::Utc::now()),
                    doc_date_end: None,
                    title: Some(format!("title {i}")),
                    language: None,
                    page_count: 1,
                })
                .unwrap();
            v.add_ocr(NewOcr {
                document_id: doc.id,
                page_no: 1,
                backend: OcrBackendKind::Native,
                model_version: "text-layer".into(),
                text: format!("needle{i} common"),
                confidence: None,
            })
            .unwrap();
        }
        v.rebuild_encounters().unwrap();

        let before = (
            v.debug_count("source_file"),
            v.debug_count("document"),
            v.debug_count("ocr_result"),
            v.search("common", 20).unwrap().len(),
        );
        v.rebuild_from_log().unwrap();
        let after = (
            v.debug_count("source_file"),
            v.debug_count("document"),
            v.debug_count("ocr_result"),
            v.search("common", 20).unwrap().len(),
        );
        assert_eq!(before, after);
        assert_eq!(after.0, 5);
    }
}
