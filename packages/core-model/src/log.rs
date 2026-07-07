//! Append-only JSONL event log under `<vault>/log/`.
//!
//! `log/000001.jsonl`, one `LogEntry` per line. Append opens the last segment
//! (or creates the first) in append mode and writes one flushed line;
//! `read_all` concatenates all segments in name order, which is also event
//! order since segments are only ever appended to, never rewritten.

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

    fn segments(&self) -> Result<Vec<PathBuf>, MedmeError> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&self.dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
            .collect();
        files.sort();
        Ok(files)
    }

    /// Append one event line to the active (last existing, or newly created) segment.
    pub fn append(&self, entry: &LogEntry) -> Result<(), MedmeError> {
        let segments = self.segments()?;
        let path = segments
            .last()
            .cloned()
            .unwrap_or_else(|| self.dir.join("000001.jsonl"));
        let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
        let line = serde_json::to_string(entry)?;
        writeln!(f, "{line}")?;
        f.flush()?;
        Ok(())
    }

    /// All events across segments, in append order (== seq order).
    pub fn read_all(&self) -> Result<Vec<LogEntry>, MedmeError> {
        let mut out = Vec::new();
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
        LogEntry::new(
            seq,
            "2024-01-01T00:00:00Z".into(),
            "dev1".into(),
            Event::FileImported {
                content_hash: format!("h{seq}"),
                original_name: "a".into(),
                mime_type: "text/plain".into(),
                byte_size: 1,
                imported_at: "2024-01-01T00:00:00Z".into(),
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
}
