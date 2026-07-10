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
        Self::open_inner(root, None)
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
        Self::open_inner(root, Some(device_id))
    }

    /// Shared open logic for [`Vault::open`] and [`Vault::open_with_device_id`].
    /// The ONLY difference is the source of `device_id`: an explicit
    /// machine-local id when `device_id` is `Some`, otherwise the
    /// vault-db-stored id via `ensure_device_id`.
    fn open_inner(root: &Path, device_id: Option<&str>) -> Result<Vault, MedmeError> {
        std::fs::create_dir_all(root.join("objects"))?;
        std::fs::write(root.join("VERSION"), "1")?;
        let conn = Connection::open(root.join("medme.db"))?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        schema::migrate(&conn)?;
        schema::ensure_meta_table(&conn)?;
        schema::ensure_imaging_instance_unique_index(&conn)?;
        let log = log::EventLog::open(root)?;

        let mut vault = Vault {
            conn,
            root: root.to_path_buf(),
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
}
