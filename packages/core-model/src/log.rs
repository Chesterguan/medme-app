//! Append-only JSONL event log under `<vault>/log/`, segmented per device.
//!
//! Each device appends **only to its own segment** `log/<device_id>-000001.jsonl`
//! (one `LogEntry` per line), so multiple devices sharing a cloud-synced vault
//! never write the same file → no write conflicts (see `docs/013_Mobile_App.md`
//! §3, §6). A pre-refactor vault's single `log/000001.jsonl` is picked up as
//! just one more segment — no migration needed.
//!
//! `read_all` scans **all** `*.jsonl` segments (every device's + any legacy
//! one) and merges them into a single **deterministic global order**: by event
//! timestamp, tie-broken by `(device_id, seq)`. Because the log is append-only
//! and each entity is content-hash/uuid keyed (created once), replaying this
//! merged set reproduces a consistent state regardless of how many devices
//! contributed or in what filesystem order the segments were enumerated.

use crate::event::LogEntry;
use crate::MedmeError;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

pub struct EventLog {
    dir: PathBuf,
}

impl EventLog {
    pub fn open(vault_root: &Path) -> Result<Self, MedmeError> {
        let dir = vault_root.join("log");
        std::fs::create_dir_all(&dir)?;
        Ok(EventLog { dir })
    }

    /// All `.jsonl` segment files in the log dir: every device's per-device
    /// segment plus any legacy single `000001.jsonl`. Order is irrelevant —
    /// `read_all` sorts events globally — but we sort by name for stable I/O.
    fn segments(&self) -> Result<Vec<PathBuf>, MedmeError> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&self.dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
            .collect();
        files.sort();
        Ok(files)
    }

    /// The segment this device appends to. New events from a device go to its
    /// own file, so two devices never contend for the same segment.
    fn device_segment(&self, device_id: &str) -> PathBuf {
        self.dir.join(format!("{device_id}-000001.jsonl"))
    }

    /// Append one event line to the appending device's own segment (keyed by
    /// `entry.device_id`), creating it on first write.
    pub fn append(&self, entry: &LogEntry) -> Result<(), MedmeError> {
        let path = self.device_segment(&entry.device_id);
        let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
        let line = serde_json::to_string(entry)?;
        writeln!(f, "{line}")?;
        f.flush()?;
        // Durability: fsync the appended line so a "saved" event survives power
        // loss before we return success (medical data — silent loss is fatal).
        f.sync_all()?;
        Ok(())
    }

    /// All events across every segment, merged into a deterministic global
    /// order: primarily by timestamp, tie-broken by `(device_id, seq)`. This
    /// order is independent of filesystem enumeration, so any device rebuilds
    /// to the identical state from the same set of segments.
    pub fn read_all(&self) -> Result<Vec<LogEntry>, MedmeError> {
        let mut out: Vec<LogEntry> = Vec::new();
        for path in self.segments()? {
            let f = std::fs::File::open(&path)?;
            for line in BufReader::new(f).lines() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                out.push(serde_json::from_str(&line)?);
            }
        }
        out.sort_by(|a, b| {
            a.ts.cmp(&b.ts)
                .then_with(|| a.device_id.cmp(&b.device_id))
                .then_with(|| a.seq.cmp(&b.seq))
        });
        Ok(out)
    }

    pub fn is_empty(&self) -> Result<bool, MedmeError> {
        Ok(self.segments()?.is_empty() || self.read_all()?.is_empty())
    }

    pub fn max_seq(&self) -> Result<i64, MedmeError> {
        Ok(self.read_all()?.iter().map(|e| e.seq).max().unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Event;

    fn mk(seq: i64) -> LogEntry {
        mk_on("dev1", seq, "2024-01-01T00:00:00Z")
    }

    fn mk_on(device: &str, seq: i64, ts: &str) -> LogEntry {
        LogEntry::new(
            seq,
            ts.into(),
            device.into(),
            Event::FileImported {
                content_hash: format!("{device}-h{seq}"),
                original_name: "a".into(),
                mime_type: "text/plain".into(),
                byte_size: 1,
                imported_at: ts.into(),
            },
        )
        .unwrap()
    }

    #[test]
    fn append_and_read_all_round_trips_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let log = EventLog::open(dir.path()).unwrap();
        assert!(log.is_empty().unwrap());

        log.append(&mk(1)).unwrap();
        log.append(&mk(2)).unwrap();

        let events = log.read_all().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].seq, 1);
        assert_eq!(events[1].seq, 2);
        assert_eq!(log.max_seq().unwrap(), 2);
        assert!(!log.is_empty().unwrap());
    }

    #[test]
    fn reopen_appends_to_existing_segment() {
        let dir = tempfile::tempdir().unwrap();
        {
            let log = EventLog::open(dir.path()).unwrap();
            log.append(&mk(1)).unwrap();
        }
        let log2 = EventLog::open(dir.path()).unwrap();
        log2.append(&mk(2)).unwrap();
        assert_eq!(log2.read_all().unwrap().len(), 2);
    }

    #[test]
    fn append_writes_to_per_device_segment() {
        let dir = tempfile::tempdir().unwrap();
        let log = EventLog::open(dir.path()).unwrap();
        log.append(&mk_on("devA", 1, "2024-01-01T00:00:00Z"))
            .unwrap();
        log.append(&mk_on("devB", 1, "2024-01-01T00:00:01Z"))
            .unwrap();

        assert!(dir.path().join("log/devA-000001.jsonl").is_file());
        assert!(dir.path().join("log/devB-000001.jsonl").is_file());
        // Each device wrote only its own segment; no shared/legacy file created.
        assert!(!dir.path().join("log/000001.jsonl").exists());
        assert_eq!(log.read_all().unwrap().len(), 2);
    }

    #[test]
    fn read_all_merges_segments_in_deterministic_ts_order_not_filename_order() {
        let dir = tempfile::tempdir().unwrap();
        let log_dir = dir.path().join("log");
        std::fs::create_dir_all(&log_dir).unwrap();

        // Two segments whose filename alpha order (aaa < zzz) is the OPPOSITE
        // of their events' timestamp order — so name-order concatenation would
        // misorder them; the (ts, device_id, seq) sort must not.
        let e_late = mk_on("zzz", 1, "2024-06-01T00:00:00Z");
        let e_early = mk_on("aaa", 1, "2024-01-01T00:00:00Z");
        std::fs::write(
            log_dir.join("zzz-000001.jsonl"),
            format!("{}\n", serde_json::to_string(&e_late).unwrap()),
        )
        .unwrap();
        std::fs::write(
            log_dir.join("aaa-000001.jsonl"),
            format!("{}\n", serde_json::to_string(&e_early).unwrap()),
        )
        .unwrap();

        let log = EventLog::open(dir.path()).unwrap();
        let events = log.read_all().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0].ts, "2024-01-01T00:00:00Z",
            "earlier ts sorts first"
        );
        assert_eq!(events[1].ts, "2024-06-01T00:00:00Z");
    }

    #[test]
    fn legacy_single_log_is_picked_up_as_one_segment() {
        let dir = tempfile::tempdir().unwrap();
        let log_dir = dir.path().join("log");
        std::fs::create_dir_all(&log_dir).unwrap();
        // A pre-refactor vault: one plain, non-namespaced segment file.
        let e = mk_on("legacydev", 1, "2023-01-01T00:00:00Z");
        std::fs::write(
            log_dir.join("000001.jsonl"),
            format!("{}\n", serde_json::to_string(&e).unwrap()),
        )
        .unwrap();

        let log = EventLog::open(dir.path()).unwrap();
        assert!(!log.is_empty().unwrap());
        let events = log.read_all().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].device_id, "legacydev");
    }
}
