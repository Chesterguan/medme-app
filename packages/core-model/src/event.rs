//! Event types for the append-only log (see `docs/011_Storage_Sync.md`).
//!
//! One variant per existing `Vault` write op, kept granular so replaying the
//! log reproduces exactly what the write methods did. Events reference other
//! rows by content hash (never by DB autoincrement id) so they stay valid
//! independent of any particular SQLite database.

use crate::MedmeError;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// Genesis `prev_hash` for the first entry of a log segment: 64 hex zeros. A
/// well-formed chain therefore always starts here, so a segment whose first
/// entry's `prev_hash` is anything else has been truncated at the head.
pub const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

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
    /// 影像切片挂载到一个「影像检查(imaging study)」文档上(imaging overhaul P1)。
    /// 一个 DICOM 实例(切片)进 CAS 后,按 `study_uid` 归入同一 study 文档:第一个
    /// 实例经 `DocumentAdded` 建文档,其后同 study 的实例只 append 本事件(不建新文档)。
    ///
    /// 与本模块其它事件一致,用内容哈希(而非 DB 自增 id)引用行,保证脱离具体
    /// SQLite 库仍可重放:`document_ref` 指向 study 文档的锚点 source_file(即第一个
    /// 实例的 source_file),`source_file_hash` 指向本切片自己的 source_file。
    /// materialize 时顺带把 `study_uid` 落到该文档(`document.study_uid`),供
    /// study→document 查找;因此 `DocumentAdded` 无需新增字段。
    ImagingInstanceAdded {
        document_ref: DocRef,
        source_file_hash: String,
        study_uid: String,
        series_uid: Option<String>,
        series_number: Option<i32>,
        instance_number: Option<i32>,
        created_at: String,
    },
    /// 审计事件:一次导出(如时间线 HTML)。对 DB 投影是纯粹的 no-op —— 只留痕
    /// 供 `Vault::audit_log()` 展示,`apply_event`/`rebuild_from_log` 必须忽略它。
    ExportPerformed {
        at: String,
        kind: String,
        record_count: i64,
        sha256: String,
    },
    /// 审计事件:一次加密分享。同上,对 DB 投影是 no-op。
    ShareCreated {
        at: String,
        record_count: i64,
        sha256: String,
        expires: String,
    },
    /// 删除一份文档(用户在 UI 里移除)。按锚点 source_file 的**内容哈希**引用,脱库
    /// 重放稳定(不用 DB 自增 id)。materialize 时移除该文档的派生行
    /// (document / ocr_result / imaging_instance / FTS);**原始字节留在 CAS 不动**
    /// —— Raw Never Dies + 同步安全,删除只是墓碑。删除作为事件同步,各端重放后一致。
    DocumentDeleted {
        source_file_hash: String,
        deleted_at: String,
    },
}

/// One line in the append-only log: an `Event` plus the envelope needed for
/// ordering, dedup, and (future) sync.
///
/// `prev_hash` + `mac` make the synced log tamper-evident and authenticated
/// (advisory GHSA-m96x). `prev_hash` chains each entry to the previous one in
/// the SAME segment (sha256 of the previous entry's canonical bytes; genesis =
/// [`GENESIS_HASH`]) so insert/delete/reorder are detectable. `mac` is an
/// HMAC-SHA256 over the entry's canonical bytes keyed by the per-vault secret,
/// so a shared-folder writer without the key cannot forge or tamper an entry
/// undetected. Both are `Option` and `#[serde(default)]` so legacy logs written
/// before this change still deserialize (they migrate on open — see
/// `log::EventLog::migrate_and_seal`).
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
    /// sha256 of the previous entry's canonical bytes in this segment
    /// (`GENESIS_HASH` for the first). `None` only for a not-yet-migrated
    /// legacy entry.
    #[serde(default)]
    pub prev_hash: Option<String>,
    /// HMAC-SHA256 (hex) over this entry's canonical bytes, keyed by the
    /// per-vault secret. `None` when written without a key (chain-only mode)
    /// or by a legacy build.
    #[serde(default)]
    pub mac: Option<String>,
}

impl LogEntry {
    /// Build an UNSEALED entry (`prev_hash`/`mac` unset). Sealing — computing
    /// the chain link and MAC — happens at append time in
    /// `log::EventLog::append`, which knows the segment tail and the key.
    pub fn new(seq: i64, ts: String, device_id: String, event: Event) -> Result<Self, MedmeError> {
        Ok(LogEntry {
            event_id: event_id(&event)?,
            seq,
            ts,
            device_id,
            event,
            prev_hash: None,
            mac: None,
        })
    }

    /// THE canonical serialization of the authenticated fields — the single
    /// source of bytes for BOTH the chain hash and the MAC, so the two can
    /// never drift. Serde emits `Canonical`'s fields in declaration order (the
    /// same determinism `event_id` already relies on), giving a stable,
    /// reproducible byte string. A missing `prev_hash` folds to `GENESIS_HASH`
    /// so a legacy entry hashes identically before and after migration.
    pub(crate) fn canonical_bytes(&self) -> Result<Vec<u8>, MedmeError> {
        #[derive(Serialize)]
        struct Canonical<'a> {
            seq: i64,
            ts: &'a str,
            device_id: &'a str,
            prev_hash: &'a str,
            event: &'a Event,
        }
        let bytes = serde_json::to_vec(&Canonical {
            seq: self.seq,
            ts: &self.ts,
            device_id: &self.device_id,
            prev_hash: self.prev_hash.as_deref().unwrap_or(GENESIS_HASH),
            event: &self.event,
        })?;
        Ok(bytes)
    }

    /// The chain link this entry contributes: sha256 of its canonical bytes.
    /// The next entry in the segment stores this as its `prev_hash`.
    pub(crate) fn chain_hash(&self) -> Result<String, MedmeError> {
        Ok(crate::cas::sha256_hex(&self.canonical_bytes()?))
    }

    /// HMAC-SHA256 (hex) of this entry's canonical bytes under `key`.
    pub(crate) fn compute_mac(&self, key: &[u8]) -> Result<String, MedmeError> {
        // `new_from_slice` only errors on key lengths HMAC can't accept; HMAC
        // accepts ANY length (it hashes/pads), so this never fails.
        let mut mac =
            HmacSha256::new_from_slice(key).expect("HMAC-SHA256 accepts a key of any length");
        mac.update(&self.canonical_bytes()?);
        Ok(hex(&mac.finalize().into_bytes()))
    }

    /// Constant-time verify of `self.mac` against a freshly computed MAC over
    /// the canonical bytes. `false` if the MAC is absent, malformed, or wrong.
    pub(crate) fn verify_mac(&self, key: &[u8]) -> Result<bool, MedmeError> {
        let Some(stored) = self.mac.as_deref() else {
            return Ok(false);
        };
        let Some(tag) = hex_decode(stored) else {
            return Ok(false);
        };
        let mut mac =
            HmacSha256::new_from_slice(key).expect("HMAC-SHA256 accepts a key of any length");
        mac.update(&self.canonical_bytes()?);
        Ok(mac.verify_slice(&tag).is_ok())
    }

    /// Stamp `prev_hash` and (when a key is present) `mac` onto this entry,
    /// making it a sealed, verifiable log line. Called by `EventLog::append`
    /// with the segment's tail hash and the vault key, and by migration.
    pub(crate) fn seal(&mut self, prev_hash: String, key: Option<&[u8]>) -> Result<(), MedmeError> {
        self.prev_hash = Some(prev_hash);
        self.mac = match key {
            Some(k) => Some(self.compute_mac(k)?),
            None => None,
        };
        Ok(())
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

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Decode a lowercase/uppercase hex string to bytes; `None` if it isn't valid
/// even-length hex (a malformed stored MAC then simply fails verification).
fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
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
        assert_eq!(
            event_id(&imported(3)).unwrap(),
            event_id(&imported(3)).unwrap()
        );
        assert_ne!(
            event_id(&imported(3)).unwrap(),
            event_id(&imported(4)).unwrap()
        );
    }

    #[test]
    fn log_entry_round_trips_through_json() {
        let entry =
            LogEntry::new(1, "2024-01-01T00:00:00Z".into(), "dev1".into(), imported(3)).unwrap();
        let line = serde_json::to_string(&entry).unwrap();
        let back: LogEntry = serde_json::from_str(&line).unwrap();
        assert_eq!(back.event_id, entry.event_id);
        assert_eq!(back.seq, entry.seq);
        assert_eq!(back.event, entry.event);
    }

    /// A truly legacy line (no `prev_hash`/`mac` keys at all) still parses,
    /// with both folding to `None` via `#[serde(default)]`.
    #[test]
    fn legacy_entry_without_prev_hash_or_mac_parses() {
        let legacy = r#"{"event_id":"x","seq":1,"ts":"2024-01-01T00:00:00Z","device_id":"dev1","type":"FileImported","content_hash":"abc","original_name":"a.pdf","mime_type":"application/pdf","byte_size":3,"imported_at":"2024-01-01T00:00:00Z"}"#;
        let e: LogEntry = serde_json::from_str(legacy).unwrap();
        assert!(e.prev_hash.is_none());
        assert!(e.mac.is_none());
    }

    const KEY: &[u8] = &[7u8; 32];

    fn entry(seq: i64) -> LogEntry {
        LogEntry::new(
            seq,
            "2024-01-01T00:00:00Z".into(),
            "dev1".into(),
            imported(seq),
        )
        .unwrap()
    }

    #[test]
    fn canonical_bytes_are_deterministic_and_prev_hash_sensitive() {
        let mut a = entry(1);
        a.prev_hash = Some(GENESIS_HASH.to_string());
        let mut b = entry(1);
        b.prev_hash = Some(GENESIS_HASH.to_string());
        assert_eq!(a.canonical_bytes().unwrap(), b.canonical_bytes().unwrap());
        // Changing prev_hash changes the canonical bytes (so the chain links).
        b.prev_hash = Some("f".repeat(64));
        assert_ne!(a.canonical_bytes().unwrap(), b.canonical_bytes().unwrap());
        // A missing prev_hash folds to GENESIS_HASH — identical bytes.
        let mut c = entry(1);
        c.prev_hash = None;
        assert_eq!(a.canonical_bytes().unwrap(), c.canonical_bytes().unwrap());
    }

    #[test]
    fn mac_round_trips_and_detects_tamper() {
        let mut e = entry(1);
        e.seal(GENESIS_HASH.to_string(), Some(KEY)).unwrap();
        assert!(e.prev_hash.is_some() && e.mac.is_some());
        assert!(e.verify_mac(KEY).unwrap(), "fresh MAC verifies");

        // Wrong key fails.
        assert!(!e.verify_mac(&[9u8; 32]).unwrap());

        // Tamper the event content → MAC no longer verifies (mac field stale).
        let mut tampered = e.clone();
        tampered.event = imported(999);
        assert!(
            !tampered.verify_mac(KEY).unwrap(),
            "tampered content fails MAC"
        );

        // Absent / malformed stored MAC fails cleanly (no panic).
        let mut nomac = e.clone();
        nomac.mac = None;
        assert!(!nomac.verify_mac(KEY).unwrap());
        nomac.mac = Some("nothex!!".into());
        assert!(!nomac.verify_mac(KEY).unwrap());
    }

    #[test]
    fn seal_without_key_sets_chain_but_no_mac() {
        let mut e = entry(1);
        e.seal(GENESIS_HASH.to_string(), None).unwrap();
        assert_eq!(e.prev_hash.as_deref(), Some(GENESIS_HASH));
        assert!(e.mac.is_none(), "chain-only seal leaves MAC unset");
    }
}
