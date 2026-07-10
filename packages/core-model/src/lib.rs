pub mod audit;
pub mod cas;
pub mod error;
pub mod event;
pub mod imaging;
pub mod log;
pub mod materialize;
pub mod query;
pub mod relocate;
pub mod schema;
pub mod tokenize;
pub mod types;

pub use audit::AuditEntry;
pub use error::MedmeError;
pub use event::{DocRef, Event};
pub use materialize::generate_device_id;
pub use query::{extract_provider, SearchHit, TimelineEntry};
pub use types::{
    DocType, Document, Encounter, EncounterKind, ImagingInstance, Import, NewDocument,
    NewImagingInstance, NewOcr, OcrBackendKind, SourceFile,
};

use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};

/// Truth = `objects/` (CAS) + `log/` (append-only event log).
/// `medme.db` is a derived cache, materialized by replaying the log; it can
/// be deleted and rebuilt (see `materialize::Vault::rebuild_from_log`).
pub struct Vault {
    conn: Connection,
    root: PathBuf,
    log: log::EventLog,
    next_seq: AtomicI64,
    device_id: String,
}

impl Vault {
    /// Open the vault at `root`, taking its `device_id` from the vault db
    /// (`ensure_device_id`: read the stored id, or generate + persist one on
    /// first open). Correct for single-machine use.
    ///
    /// For a vault folder SHARED across machines (multi-device cloud sync), use
    /// [`Vault::open_with_device_id`] instead: the db-stored id lives inside the
    /// shared folder, so every machine opening it here would inherit the SAME
    /// id and write to the SAME per-device log segment — defeating the
    /// conflict-free per-device segmentation.
    pub fn open(root: &Path) -> Result<Vault, MedmeError> {
        Self::open_inner(root, &root.join("medme.db"), None)
    }

    /// Like [`Vault::open`], but forces `device_id` to the given machine-local
    /// id instead of reading/generating it from the vault db. This is what makes
    /// shared-folder multi-device sync conflict-free: each machine passes its
    /// OWN persistent id (stored OUTSIDE the shared vault), so new log entries
    /// are stamped with it and this machine's log segment is namespaced
    /// `log/<device_id>-*.jsonl` — never colliding with another machine's
    /// segment. Existing segments (written under whatever id) are untouched and
    /// still read back by `read_all`.
    pub fn open_with_device_id(root: &Path, device_id: &str) -> Result<Vault, MedmeError> {
        Self::open_inner(root, &root.join("medme.db"), Some(device_id))
    }

    /// Open a vault whose **truth** (`objects/` + `log/`) lives under
    /// `truth_root` but whose **derived** SQLite db lives at a SEPARATE
    /// `db_path`. This is what iOS iCloud sync needs: `objects/` + `log/` go
    /// into the iCloud ubiquity container (a real POSIX path iCloud syncs
    /// across the user's Apple devices), while `medme.db` stays LOCAL in the
    /// app sandbox and is rebuilt from the log — never placing SQLite on
    /// iCloud (which would corrupt it). See `docs/011_Storage_Sync.md`.
    ///
    /// `truth_root/{objects,log,VERSION}` and `db_path` are created if absent
    /// (the db is materialized from the log on open, so a missing/deleted
    /// `db_path` simply rebuilds). `cas::root()` returns `truth_root`, so CAS
    /// and the event log resolve there; only the db connection uses `db_path`.
    /// Like [`Vault::open_with_device_id`], the id is forced machine-local so a
    /// shared truth folder stays conflict-free (per-device log segments).
    pub fn open_split(
        truth_root: &Path,
        db_path: &Path,
        device_id: &str,
    ) -> Result<Vault, MedmeError> {
        Self::open_inner(truth_root, db_path, Some(device_id))
    }

    /// Shared open logic for [`Vault::open`], [`Vault::open_with_device_id`] and
    /// [`Vault::open_split`]. Two things vary:
    /// - `db_path`: where the derived SQLite db is opened. Defaults to
    ///   `<truth_root>/medme.db` for the non-split entrypoints; `open_split`
    ///   points it elsewhere (e.g. the app sandbox, off iCloud).
    /// - `device_id`: an explicit machine-local id when `Some`, otherwise the
    ///   vault-db-stored id via `ensure_device_id`.
    fn open_inner(
        truth_root: &Path,
        db_path: &Path,
        device_id: Option<&str>,
    ) -> Result<Vault, MedmeError> {
        std::fs::create_dir_all(truth_root.join("objects"))?;
        std::fs::write(truth_root.join("VERSION"), "1")?;
        // The db may live outside `truth_root` (split mode); make sure its
        // parent directory exists before SQLite tries to create the file.
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path)?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        schema::migrate(&conn)?;
        schema::ensure_meta_table(&conn)?;
        schema::ensure_imaging_instance_unique_index(&conn)?;
        let log = log::EventLog::open(truth_root)?;

        let mut vault = Vault {
            conn,
            root: truth_root.to_path_buf(),
            log,
            next_seq: AtomicI64::new(1),
            device_id: String::new(),
        };
        vault.device_id = match device_id {
            Some(id) => id.to_string(),
            None => vault.ensure_device_id()?,
        };

        let log_is_empty = vault.log.is_empty()?;
        let has_existing_rows: i64 =
            vault
                .conn
                .query_row("SELECT COUNT(*) FROM source_file", [], |r| r.get(0))?;
        if log_is_empty && has_existing_rows > 0 {
            // Pre-refactor, DB-only vault: synthesize the log from current DB
            // rows and mark it as fully applied — the DB already reflects it.
            vault.migrate_db_to_log()?;
        } else {
            // Fresh vault (both empty) or a normal reopen: apply anything
            // past the watermark. No-op for a fresh vault.
            vault.materialize()?;
        }
        let max_seq = vault.log.max_seq()?;
        vault.next_seq.store(max_seq + 1, Ordering::SeqCst);
        Ok(vault)
    }

    pub fn user_version(&self) -> Result<i64, MedmeError> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    pub(crate) fn conn(&self) -> &rusqlite::Connection {
        &self.conn
    }

    /// Allocate the next log sequence number (monotonically increasing for
    /// the lifetime of this open `Vault`; reinitialized from the log's max
    /// seq on every `open`).
    pub(crate) fn next_seq(&self) -> i64 {
        self.next_seq.fetch_add(1, Ordering::SeqCst)
    }

    /// Append one event to the log. Does not materialize — callers apply it
    /// with `self.materialize()` (or, during one-time DB→log migration,
    /// leave it unapplied and advance the watermark instead).
    pub(crate) fn append_event(&self, event: event::Event) -> Result<(), MedmeError> {
        let seq = self.next_seq();
        let ts = Self::now_rfc3339();
        let entry = event::LogEntry::new(seq, ts, self.device_id.clone(), event)?;
        self.log.append(&entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_creates_vault_and_migrates() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        assert_eq!(v.user_version().unwrap(), 5);
        assert!(dir.path().join("objects").is_dir());
        assert!(dir.path().join("medme.db").is_file());
        assert!(dir.path().join("log").is_dir());
    }

    #[test]
    fn reopen_is_idempotent_and_keeps_device_id() {
        let dir = tempfile::tempdir().unwrap();
        let id1 = {
            let v = Vault::open(dir.path()).unwrap();
            v.device_id.clone()
        };
        let id2 = {
            let v = Vault::open(dir.path()).unwrap();
            v.device_id.clone()
        };
        assert_eq!(id1, id2);
        assert!(!id1.is_empty());
    }

    // ---- split truth-root / db-path (iCloud sync: truth on iCloud, db local) --

    use crate::types::{NewDocument, NewOcr};
    use crate::{DocType, OcrBackendKind};

    /// Seed one imported + OCR'd doc via an already-open split vault. The OCR
    /// body is `needle` (searchable); returns the document id.
    fn seed_split_doc(v: &Vault, name: &str, needle: &str) -> i64 {
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
        doc.id
    }

    /// (a) `open_split` puts `objects/` + `log/` under `truth_root` and the
    /// SQLite db at a SEPARATE `db_path` (NOT under `truth_root`); documents +
    /// full-text search survive a reopen at the same split paths.
    #[test]
    fn open_split_keeps_db_outside_truth_root() {
        let truth = tempfile::tempdir().unwrap();
        let db_home = tempfile::tempdir().unwrap();
        let db_path = db_home.path().join("local").join("medme.db");

        {
            let v = Vault::open_split(truth.path(), &db_path, "deviceX").unwrap();
            assert_eq!(v.root(), truth.path(), "CAS/log root is the truth root");
            seed_split_doc(&v, "labs.txt", "SplitCreatinineNeedle");
        }

        // Truth (objects/ + log/) lives under truth_root; the db does NOT.
        assert!(truth.path().join("objects").is_dir(), "objects under truth");
        assert!(truth.path().join("log").is_dir(), "log under truth");
        assert!(
            !truth.path().join("medme.db").exists(),
            "db must NOT be under the truth root in split mode"
        );
        assert!(db_path.is_file(), "db materialized at the separate db_path");

        // Reopen at the same split paths: document + search intact.
        let v = Vault::open_split(truth.path(), &db_path, "deviceX").unwrap();
        assert_eq!(v.debug_count("document"), 1);
        assert_eq!(v.debug_count("source_file"), 1);
        assert_eq!(v.search("SplitCreatinineNeedle", 10).unwrap().len(), 1);
    }

    /// (b) Deleting the db at `db_path` and reopening via `open_split` rebuilds
    /// it purely from the truth root's log + CAS — proving the db is a derived,
    /// relocatable cache (exactly the iCloud story: truth syncs, db is local
    /// and disposable).
    #[test]
    fn open_split_rebuilds_deleted_db_from_truth_log() {
        let truth = tempfile::tempdir().unwrap();
        let db_home = tempfile::tempdir().unwrap();
        let db_path = db_home.path().join("medme.db");

        {
            let v = Vault::open_split(truth.path(), &db_path, "deviceX").unwrap();
            seed_split_doc(&v, "labs.txt", "RebuiltGlucoseNeedle");
            assert_eq!(v.debug_count("document"), 1);
        } // drop: close the sqlite connection before deleting the db file

        // Delete ONLY the local db; the truth (objects/ + log/) is untouched.
        std::fs::remove_file(&db_path).unwrap();
        assert!(!db_path.exists());

        // Reopen: the db is recreated and re-materialized from the log alone.
        let v = Vault::open_split(truth.path(), &db_path, "deviceX").unwrap();
        assert!(db_path.is_file(), "db recreated at db_path");
        assert_eq!(
            v.debug_count("document"),
            1,
            "document rebuilt from the truth log"
        );
        assert_eq!(v.debug_count("source_file"), 1);
        assert_eq!(
            v.search("RebuiltGlucoseNeedle", 10).unwrap().len(),
            1,
            "search index rebuilt from log + CAS"
        );
    }
}
