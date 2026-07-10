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
                    // Cap this device's watermark below the deferred seq so the
                    // event is retried next time — even if a HIGHER-seq event of
                    // the same device was already applied this pass. That can
                    // happen if the device's clock regressed (NTP step / manual
                    // set): a higher seq then carries an earlier `ts` and sorts
                    // first. Without this cap the watermark could sit above the
                    // deferred seq and the event would be dropped forever.
                    let cur = new_map.entry(entry.device_id.clone()).or_insert(0);
                    *cur = (*cur).min(entry.seq - 1);
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

/// Generate a fresh, random device id (sha256 of nanos + pid). Used both by
/// `ensure_device_id` (db-stored id) and by apps that persist a machine-local
/// id OUTSIDE the vault for [`Vault::open_with_device_id`].
pub fn generate_device_id() -> String {
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

/// Normalize a `mime_type` to a strict media-type shape (`type/subtype`, no spaces,
/// quotes, `<`, `>`). A value that doesn't match — e.g. one smuggled in via a forged
/// log event to attack the share/export `<img src>` sinks or to inject the logs —
/// falls back to `application/octet-stream` rather than aborting the import.
fn sanitize_mime_type(mime: &str) -> String {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9!#$&^_.+-]*/[A-Za-z0-9][A-Za-z0-9!#$&^_.+-]*$")
            .expect("mime shape regex")
    });
    if re.is_match(mime) {
        mime.to_string()
    } else {
        "application/octet-stream".to_string()
    }
}

fn apply_event(tx: &Transaction, vault: &Vault, event: &Event) -> Result<ApplyOutcome, MedmeError> {
    match event {
        Event::FileImported {
            content_hash,
            original_name,
            mime_type,
            byte_size,
            imported_at,
        } => {
            // SECURITY: `content_hash` from a forged log event is attacker-controlled
            // and gets sliced into a CAS path by `object_relpath`. Quarantine (skip) a
            // malformed hash rather than panic / escape `objects/` — one bad event must
            // never abort the whole replay.
            if !cas::is_object_hash(content_hash) {
                eprintln!("[materialize] skip FileImported: malformed content_hash");
                return Ok(ApplyOutcome::Applied);
            }
            let relpath = cas::object_relpath(content_hash);
            // Defense-in-depth behind the share/export XSS fix: normal imports go
            // through the `mime_for` whitelist, but a forged event can put anything in
            // `mime_type` (a value crafted to break out of `<img src>` or to injection
            // the logs). Normalize to a strict media-type shape, else fall back to
            // `application/octet-stream` — never abort the import.
            let mime_type = sanitize_mime_type(mime_type);
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
            // A forged event — or a legitimately not-yet-synced FileImported — may
            // reference a source_file that isn't in the DB. Defer instead of letting
            // `QueryReturnedNoRows` abort the whole materialize / `Vault::open` (the
            // same treatment the missing-CAS-blob path below already uses).
            let source_file_id: i64 = match tx
                .query_row(
                    "SELECT id FROM source_file WHERE content_hash = ?1",
                    [source_file_hash],
                    |r| r.get(0),
                )
                .optional()?
            {
                Some(id) => id,
                None => return Ok(ApplyOutcome::Deferred),
            };
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
            // Defer (not abort) when the referenced document isn't materialized yet —
            // a forged ref or a not-yet-synced DocumentAdded — mirroring the missing
            // source_file / missing CAS-blob handling.
            let document_id: i64 = match tx
                .query_row(
                    "SELECT d.id FROM document d JOIN source_file sf ON d.source_file_id = sf.id
                     WHERE sf.content_hash = ?1",
                    [&document_ref.source_file_hash],
                    |r| r.get(0),
                )
                .optional()?
            {
                Some(id) => id,
                None => return Ok(ApplyOutcome::Deferred),
            };
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
            // SECURITY: `text_hash` from a forged event is sliced into a CAS path.
            // Quarantine a malformed hash (skip this OCR row) rather than panic /
            // escape `objects/`; the rest of the replay continues.
            if !cas::is_object_hash(text_hash) {
                eprintln!("[materialize] skip OcrAdded: malformed text_hash");
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
            // Defer (not abort) when either referenced row isn't materialized yet —
            // a forged ImagingInstanceAdded ref or a not-yet-synced document /
            // source_file — mirroring the DocumentAdded / OcrAdded branches. A bare
            // `?` here would turn one dangling event into a whole-replay failure
            // (Vault::open error), the exact DoS the surrounding hardening prevents.
            let document_id: i64 = match tx
                .query_row(
                    "SELECT d.id FROM document d JOIN source_file sf ON d.source_file_id = sf.id
                     WHERE sf.content_hash = ?1",
                    [&document_ref.source_file_hash],
                    |r| r.get(0),
                )
                .optional()?
            {
                Some(id) => id,
                None => return Ok(ApplyOutcome::Deferred),
            };
            let source_file_id: i64 = match tx
                .query_row(
                    "SELECT id FROM source_file WHERE content_hash = ?1",
                    [source_file_hash],
                    |r| r.get(0),
                )
                .optional()?
            {
                Some(id) => id,
                None => return Ok(ApplyOutcome::Deferred),
            };
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

        // Simulate a pre-refactor, DB-only vault: drop the log + clear the
        // per-device watermark map (the current watermark key) so nothing is
        // marked already-applied.
        std::fs::remove_dir_all(dir.path().join("log")).unwrap();
        {
            let conn = rusqlite::Connection::open(dir.path().join("medme.db")).unwrap();
            conn.execute("DELETE FROM meta WHERE key = 'applied_seq_map'", [])
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
        assert_eq!(
            v.debug_count("source_file"),
            1,
            "FileImported still applied"
        );
        assert_eq!(v.debug_count("document"), 1, "DocumentAdded still applied");
        assert_eq!(
            v.debug_count("ocr_result"),
            0,
            "OCR deferred while its object is missing"
        );

        // The blob arrives; re-materialize → the deferred OCR now applies.
        std::fs::write(&obj, &saved).unwrap();
        v.materialize().unwrap();
        assert_eq!(
            v.debug_count("ocr_result"),
            1,
            "deferred OCR applied once blob present"
        );
        assert_eq!(v.search("MissingObjNeedle", 10).unwrap().len(), 1);
    }

    /// M3 regression: a deferred (missing-CAS) event must not be stranded even
    /// when a HIGHER-seq event of the same device sorts before it — which happens
    /// if the device's clock regresses so a later append carries an earlier `ts`.
    /// The deferred device's watermark must be capped below the deferred seq, or
    /// the higher seq pushes it past the gap and the event is dropped forever.
    #[test]
    fn deferred_ocr_not_stranded_when_higher_seq_sorts_first() {
        use crate::event::{DocRef, Event, LogEntry};
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        let dev = v.device_id.clone();
        let sfh = cas::sha256_hex(b"anchor-file");
        let append = |seq: i64, ts: &str, ev: Event| {
            v.log
                .append(&LogEntry::new(seq, ts.into(), dev.clone(), ev).unwrap())
                .unwrap();
        };
        // Anchor file (seq 1) + a 2-page document (seq 2).
        append(
            1,
            "2024-01-01T00:00:10Z",
            Event::FileImported {
                content_hash: sfh.clone(),
                original_name: "a.txt".into(),
                mime_type: "text/plain".into(),
                byte_size: 6,
                imported_at: "2024-01-01T00:00:10Z".into(),
            },
        );
        append(
            2,
            "2024-01-01T00:00:11Z",
            Event::DocumentAdded {
                source_file_hash: sfh.clone(),
                doc_type: "lab_report".into(),
                doc_date: None,
                doc_date_end: None,
                title: Some("c".into()),
                language: None,
                page_count: 2,
                created_at: "2024-01-01T00:00:11Z".into(),
            },
        );
        // page-2 OCR: seq 4, blob PRESENT, EARLIER ts (clock regressed on append).
        let (h2, _, _) = v.store_object(b"Page2PresentNeedle").unwrap();
        append(
            4,
            "2024-01-01T00:00:12Z",
            Event::OcrAdded {
                document_ref: DocRef {
                    source_file_hash: sfh.clone(),
                },
                page_no: 2,
                backend: "native".into(),
                model_version: "m".into(),
                text_hash: h2,
                confidence: None,
                created_at: "2024-01-01T00:00:12Z".into(),
            },
        );
        // page-1 OCR: seq 3 (LOWER), blob MISSING, LATER ts → sorts AFTER seq 4.
        let h1 = cas::sha256_hex(b"Page1MissingNeedle");
        append(
            3,
            "2024-01-01T00:00:13Z",
            Event::OcrAdded {
                document_ref: DocRef {
                    source_file_hash: sfh.clone(),
                },
                page_no: 1,
                backend: "native".into(),
                model_version: "m".into(),
                text_hash: h1,
                confidence: None,
                created_at: "2024-01-01T00:00:13Z".into(),
            },
        );

        // First pass: seq 4 (page 2) applies; seq 3 (page 1) defers on its missing blob.
        v.materialize().unwrap();
        assert_eq!(
            v.debug_count("ocr_result"),
            1,
            "only the present page applied"
        );

        // The missing blob arrives → re-materialize. Without the watermark cap the
        // higher seq (4) would have pushed the watermark past 3 and page 1 would be
        // dropped forever; with the cap it recovers.
        v.store_object(b"Page1MissingNeedle").unwrap();
        v.materialize().unwrap();
        assert_eq!(
            v.debug_count("ocr_result"),
            2,
            "deferred page recovered — not stranded by the higher-seq event"
        );
    }

    // ---- hardening: forged / dangling references + malformed hashes ---------

    /// A forged (or not-yet-synced) event that references a hash never present in
    /// the DB must DEFER, not abort `materialize` / `Vault::open` with
    /// `QueryReturnedNoRows`.
    #[test]
    fn event_with_unknown_reference_defers_not_errors() {
        use crate::event::{Event, LogEntry};
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        let dev = v.device_id.clone();
        let unknown = cas::sha256_hex(b"never-imported");
        v.log
            .append(
                &LogEntry::new(
                    1,
                    "2024-01-01T00:00:00Z".into(),
                    dev,
                    Event::DocumentAdded {
                        source_file_hash: unknown,
                        doc_type: "lab_report".into(),
                        doc_date: None,
                        doc_date_end: None,
                        title: Some("forged".into()),
                        language: None,
                        page_count: 1,
                        created_at: "2024-01-01T00:00:00Z".into(),
                    },
                )
                .unwrap(),
            )
            .unwrap();

        // Must NOT error — the dangling reference is deferred.
        v.materialize().unwrap();
        assert_eq!(
            v.debug_count("document"),
            0,
            "forged doc deferred, not applied"
        );

        // Reopening (which materializes again) must still succeed.
        drop(v);
        let v2 = Vault::open(dir.path()).unwrap();
        assert_eq!(v2.debug_count("document"), 0);
    }

    /// A forged (or not-yet-synced) `ImagingInstanceAdded` whose `document_ref` /
    /// `source_file_hash` isn't in the DB must DEFER — not abort `materialize` /
    /// `Vault::open` with `QueryReturnedNoRows`. Regression guard for the branch
    /// that previously used bare `query_row(...)?` while its siblings deferred.
    #[test]
    fn forged_imaging_instance_defers_not_errors() {
        use crate::event::{DocRef, Event, LogEntry};
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        let dev = v.device_id.clone();
        let unknown = cas::sha256_hex(b"imaging-never-imported");
        v.log
            .append(
                &LogEntry::new(
                    1,
                    "2024-01-01T00:00:00Z".into(),
                    dev,
                    Event::ImagingInstanceAdded {
                        document_ref: DocRef {
                            source_file_hash: unknown.clone(),
                        },
                        source_file_hash: unknown,
                        study_uid: "1.2.3".into(),
                        series_uid: None,
                        series_number: None,
                        instance_number: None,
                        created_at: "2024-01-01T00:00:00Z".into(),
                    },
                )
                .unwrap(),
            )
            .unwrap();

        // Must NOT error — the dangling refs are deferred, not aborted.
        v.materialize().unwrap();
        assert_eq!(
            v.debug_count("imaging_instance"),
            0,
            "forged imaging instance deferred, not applied"
        );

        // Reopening (materializes again) must still succeed — no whole-replay abort.
        drop(v);
        let v2 = Vault::open(dir.path()).unwrap();
        assert_eq!(v2.debug_count("imaging_instance"), 0);
    }

    /// A forged `FileImported` / `OcrAdded` whose hash is malformed (path-traversal
    /// string, too short to slice `[0..2]`/`[2..4]`) must be QUARANTINED — no panic,
    /// no `objects/` escape — and the rest of the replay must still apply.
    #[test]
    fn malformed_hash_is_quarantined_and_replay_continues() {
        use crate::event::{DocRef, Event, LogEntry};
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        let dev = v.device_id.clone();
        let append = |seq: i64, ev: Event| {
            v.log
                .append(
                    &LogEntry::new(seq, "2024-01-01T00:00:00Z".into(), dev.clone(), ev).unwrap(),
                )
                .unwrap();
        };

        // Two malformed content_hashes that would panic (byte-slice / multibyte
        // boundary) or escape objects/ if sliced into a path.
        for (seq, bad) in [(1i64, "../../etc/passwd"), (2, "abc")] {
            append(
                seq,
                Event::FileImported {
                    content_hash: bad.into(),
                    original_name: "evil".into(),
                    mime_type: "text/plain".into(),
                    byte_size: 1,
                    imported_at: "2024-01-01T00:00:00Z".into(),
                },
            );
        }
        // A VALID import after the bad ones must still land.
        let good = cas::sha256_hex(b"good bytes");
        v.store_object(b"good bytes").unwrap();
        append(
            3,
            Event::FileImported {
                content_hash: good.clone(),
                original_name: "good.txt".into(),
                mime_type: "text/plain".into(),
                byte_size: 9,
                imported_at: "2024-01-01T00:00:00Z".into(),
            },
        );
        // A malformed text_hash in an OcrAdded (referencing the valid file) must also
        // be skipped without panicking.
        append(
            4,
            Event::OcrAdded {
                document_ref: DocRef {
                    source_file_hash: good.clone(),
                },
                page_no: 1,
                backend: "native".into(),
                model_version: "m".into(),
                text_hash: "../../etc/passwd".into(),
                confidence: None,
                created_at: "2024-01-01T00:00:00Z".into(),
            },
        );

        // No panic; only the valid file applied, malformed hashes quarantined.
        v.materialize().unwrap();
        assert_eq!(
            v.debug_count("source_file"),
            1,
            "only the valid import applied; malformed hashes skipped"
        );
        assert_eq!(
            v.debug_count("ocr_result"),
            0,
            "OCR with bad text_hash skipped"
        );
    }

    /// A forged `FileImported.mime_type` that isn't a strict media-type shape (here a
    /// value crafted to break out of the share/export `<img src>` attribute) must be
    /// normalized to `application/octet-stream`, not stored verbatim.
    #[test]
    fn forged_mime_type_is_normalized_to_octet_stream() {
        use crate::event::{Event, LogEntry};
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        let dev = v.device_id.clone();
        let h = cas::sha256_hex(b"bytes");
        v.store_object(b"bytes").unwrap();
        let evil = "image/png\"><script>alert(1)</script>";
        v.log
            .append(
                &LogEntry::new(
                    1,
                    "2024-01-01T00:00:00Z".into(),
                    dev,
                    Event::FileImported {
                        content_hash: h.clone(),
                        original_name: "x".into(),
                        mime_type: evil.into(),
                        byte_size: 5,
                        imported_at: "2024-01-01T00:00:00Z".into(),
                    },
                )
                .unwrap(),
            )
            .unwrap();

        v.materialize().unwrap();
        let stored: String = v
            .conn()
            .query_row(
                "SELECT mime_type FROM source_file WHERE content_hash = ?1",
                [&h],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            stored, "application/octet-stream",
            "dangerous mime normalized"
        );
    }

    #[test]
    fn sanitize_mime_type_keeps_valid_and_rejects_dangerous() {
        assert_eq!(sanitize_mime_type("image/png"), "image/png");
        assert_eq!(sanitize_mime_type("application/dicom"), "application/dicom");
        assert_eq!(sanitize_mime_type("text/plain"), "text/plain");
        // Space, quote, angle bracket, missing subtype → normalized.
        for bad in [
            "image/png\"",
            "image/png <b>",
            "image/ png",
            "notamime",
            "",
            "a/b/c",
        ] {
            assert_eq!(
                sanitize_mime_type(bad),
                "application/octet-stream",
                "must reject {bad:?}"
            );
        }
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

    // ---- vault relocate / adopt for shared-folder sync (docs/011) -----------

    /// Relocate (MOVE) into a fresh, empty directory: every document and its
    /// full-text search must still be present after reopening the vault at the
    /// new location, and the source must no longer hold the moved data.
    #[test]
    fn relocate_to_empty_dir_preserves_all_data() {
        let src = tempfile::tempdir().unwrap();
        seed_doc(src.path(), "alpha.txt", "AlphaRelocateNeedle");

        let holder = tempfile::tempdir().unwrap();
        let new_root = holder.path().join("cloud-vault"); // does not exist → MOVE
        {
            let v = Vault::open(src.path()).unwrap();
            v.relocate_to(&new_root).unwrap();
        }

        // Source's vault data was moved out (no more objects/log at the source).
        assert!(
            !src.path().join("objects").exists(),
            "objects moved out of source"
        );
        assert!(!src.path().join("log").exists(), "log moved out of source");

        // Reopen at the new location: document + search intact.
        let v = Vault::open(&new_root).unwrap();
        assert_eq!(v.debug_count("document"), 1, "document survived the move");
        assert_eq!(v.debug_count("source_file"), 1);
        assert_eq!(
            v.search("AlphaRelocateNeedle", 10).unwrap().len(),
            1,
            "search index rebuilt intact at the new location"
        );
    }

    /// Adopt into a directory that ALREADY holds a *second device's* vault
    /// (segment + objects): after copying this device's segment/objects in and
    /// `Vault::open(new_root)` + `rebuild_from_log()`, BOTH devices' documents
    /// are present — the merge reuses the multi-device log replay, not any
    /// bespoke event dedup.
    #[test]
    fn adopt_into_shared_folder_merges_both_devices() {
        // Device A (this device). Give its segment a deterministic, distinct
        // name so it can never collide with device B's random device-id name.
        let a = tempfile::tempdir().unwrap();
        seed_doc(a.path(), "alpha.txt", "AlphaAdoptNeedle");
        let a_seg = only_segment(&a.path().join("log"));
        std::fs::rename(&a_seg, a.path().join("log/deviceA-000001.jsonl")).unwrap();

        // A shared cloud folder already populated by device B (a full vault).
        let shared = tempfile::tempdir().unwrap();
        let shared_root = shared.path().join("cloud");
        seed_doc(&shared_root, "beta.txt", "BetaAdoptNeedle");

        // Device A adopts into the shared folder (copy segments + objects in).
        {
            let v = Vault::open(a.path()).unwrap();
            v.relocate_to(&shared_root).unwrap();
        }
        // Adopt must not touch the source.
        assert!(
            a.path().join("objects").exists(),
            "adopt leaves source intact"
        );

        // Reopen the shared folder and rebuild from the merged log.
        let v = Vault::open(&shared_root).unwrap();
        v.rebuild_from_log().unwrap();
        assert_eq!(v.debug_count("document"), 2, "both devices' docs present");
        assert_eq!(v.debug_count("source_file"), 2);
        assert_eq!(v.search("AlphaAdoptNeedle", 10).unwrap().len(), 1);
        assert_eq!(v.search("BetaAdoptNeedle", 10).unwrap().len(), 1);
    }

    /// `copy_to` forks the vault to a new location and LEAVES THE SOURCE INTACT
    /// (used to disable iCloud sync: copy the container vault back to local,
    /// keep the iCloud copy). After copying into a fresh dir, BOTH the source
    /// and the target hold the full vault; the target rebuilds usably from the
    /// copied log (no `medme.db` is copied).
    #[test]
    fn copy_to_forks_vault_leaving_source_intact() {
        let src = tempfile::tempdir().unwrap();
        seed_doc(src.path(), "alpha.txt", "AlphaCopyNeedle");

        let holder = tempfile::tempdir().unwrap();
        let local = holder.path().join("local-vault"); // fresh → copy in
        {
            let v = Vault::open(src.path()).unwrap();
            v.copy_to(&local).unwrap();
        }

        // Source untouched — the iCloud copy is preserved.
        assert!(
            src.path().join("objects").exists(),
            "copy_to leaves source objects"
        );
        assert!(src.path().join("log").exists(), "copy_to leaves source log");
        let vs = Vault::open(src.path()).unwrap();
        assert_eq!(vs.debug_count("document"), 1, "source still holds its doc");

        // Target holds the full vault after rebuild from the copied log.
        let v = Vault::open(&local).unwrap();
        v.rebuild_from_log().unwrap();
        assert_eq!(v.debug_count("document"), 1, "doc copied to target");
        assert_eq!(v.debug_count("source_file"), 1);
        assert_eq!(
            v.search("AlphaCopyNeedle", 10).unwrap().len(),
            1,
            "search rebuilt intact at the target"
        );
    }

    // ---- machine-local device_id: shared-folder sync must not collide -------

    /// Two machines share ONE vault folder (cloud drive). Each opens it with its
    /// OWN machine-local id via `open_with_device_id` and writes a doc. Because
    /// the id is per-machine (not read from the shared db), each machine appends
    /// to its OWN `log/<device_id>-*.jsonl` segment — no shared-segment collision
    /// — and after a rebuild BOTH documents are present + searchable.
    #[test]
    fn open_with_device_id_gives_each_machine_its_own_segment() {
        let dir = tempfile::tempdir().unwrap();

        // Machine A opens the shared folder with its own id and writes a doc.
        {
            let v = Vault::open_with_device_id(dir.path(), "machineA").unwrap();
            assert_eq!(
                v.device_id, "machineA",
                "forced machine-local id, not db id"
            );
            let imp = v
                .import("a.txt", "text/plain", b"AlphaMachineNeedle")
                .unwrap();
            let doc = v
                .add_document(NewDocument {
                    source_file_id: imp.source_file.id,
                    doc_type: DocType::LabReport,
                    doc_date: Some(chrono::Utc::now()),
                    doc_date_end: None,
                    title: Some("alpha".into()),
                    language: None,
                    page_count: 1,
                })
                .unwrap();
            v.add_ocr(NewOcr {
                document_id: doc.id,
                page_no: 1,
                backend: OcrBackendKind::Native,
                model_version: "text-layer".into(),
                text: "AlphaMachineNeedle".into(),
                confidence: None,
            })
            .unwrap();
        } // drop: close A's sqlite connection before machine B opens the folder

        // Machine B opens the SAME folder with a DIFFERENT machine-local id.
        // Under the bug (db-stored id) it would inherit "machineA"; here it must
        // keep "machineB" and write to its own segment.
        {
            let v = Vault::open_with_device_id(dir.path(), "machineB").unwrap();
            assert_eq!(v.device_id, "machineB", "second machine keeps its own id");
            let imp = v
                .import("b.txt", "text/plain", b"BetaMachineNeedle")
                .unwrap();
            let doc = v
                .add_document(NewDocument {
                    source_file_id: imp.source_file.id,
                    doc_type: DocType::LabReport,
                    doc_date: Some(chrono::Utc::now()),
                    doc_date_end: None,
                    title: Some("beta".into()),
                    language: None,
                    page_count: 1,
                })
                .unwrap();
            v.add_ocr(NewOcr {
                document_id: doc.id,
                page_no: 1,
                backend: OcrBackendKind::Native,
                model_version: "text-layer".into(),
                text: "BetaMachineNeedle".into(),
                confidence: None,
            })
            .unwrap();
        }

        // The log dir now holds TWO distinct per-machine segments, not one shared.
        let segments: Vec<String> = std::fs::read_dir(dir.path().join("log"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".jsonl"))
            .collect();
        assert!(
            segments.iter().any(|n| n.starts_with("machineA-")),
            "machineA segment present, got {segments:?}"
        );
        assert!(
            segments.iter().any(|n| n.starts_with("machineB-")),
            "machineB segment present (not merged into A's), got {segments:?}"
        );
        assert_eq!(
            segments.len(),
            2,
            "exactly two distinct per-machine segments, got {segments:?}"
        );

        // Rebuilding purely from the merged log + CAS yields BOTH documents.
        let v = Vault::open_with_device_id(dir.path(), "machineB").unwrap();
        v.rebuild_from_log().unwrap();
        assert_eq!(v.debug_count("document"), 2, "both machines' docs present");
        assert_eq!(v.debug_count("source_file"), 2);
        assert_eq!(v.search("AlphaMachineNeedle", 10).unwrap().len(), 1);
        assert_eq!(v.search("BetaMachineNeedle", 10).unwrap().len(), 1);
    }
}
