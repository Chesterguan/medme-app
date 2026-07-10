//! iCloud ubiquity-container bridge (iOS only).
//!
//! The vault's truth (`objects/` + `log/`) can live in the app's iCloud
//! ubiquity container so it syncs across the user's Apple devices, while
//! `medme.db` stays local in the sandbox and is rebuilt from the log (see
//! `core_model::Vault::open_split` and `docs/011_Storage_Sync.md`).
//!
//! ## Swift↔Rust bridge
//! `gen/apple/Sources/medme-mobile/ICloud.swift` exposes three C-ABI symbols
//! via `@_cdecl`. The iOS app target compiles that Swift source and links
//! Rust's `libapp.a`, so these `extern "C"` references resolve against the
//! Swift symbols in the same binary — no Tauri Swift plugin, no CocoaPods/SPM
//! (mirrors `vision.rs`). On any non-iOS target this whole module is
//! `#[cfg(target_os = "ios")]`-gated out; the enable/read paths that use it are
//! likewise gated, falling back to the plain local vault.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::{Path, PathBuf};

extern "C" {
    /// Returns a malloc'd, NUL-terminated UTF-8 path to the iCloud ubiquity
    /// container, or null if iCloud is unavailable / not signed in. Must be
    /// freed with `medme_icloud_free`. Runs its blocking OS lookup off the main
    /// thread (see the Swift side).
    fn medme_icloud_container_path() -> *mut c_char;
    /// Best-effort trigger a download of an evicted ubiquitous item. Returns
    /// true if the request was accepted (download is asynchronous).
    fn medme_icloud_ensure_downloaded(path: *const c_char) -> bool;
    fn medme_icloud_free(ptr: *mut c_char);
}

/// The app's iCloud ubiquity container path, or `None` if iCloud is
/// unavailable / the user is not signed in. Callers treat `None` as "iCloud not
/// available" and fall back to the local sandbox vault — never fatal.
pub fn container_path() -> Option<PathBuf> {
    // SAFETY: `medme_icloud_container_path` returns either null or a malloc'd
    // NUL-terminated C string. We copy it into an owned String, then free it
    // exactly once via the matching `medme_icloud_free`; the pointer is not
    // used afterwards.
    unsafe {
        let ptr = medme_icloud_container_path();
        if ptr.is_null() {
            return None;
        }
        let s = CStr::from_ptr(ptr).to_string_lossy().into_owned();
        medme_icloud_free(ptr);
        if s.is_empty() {
            None
        } else {
            Some(PathBuf::from(s))
        }
    }
}

/// Best-effort ask iCloud to download an evicted (dataless placeholder) file so
/// a subsequent read can see its bytes. Returns false on any error (a NUL in
/// the path, or the OS rejecting the request); callers treat that as
/// "couldn't trigger a download" and proceed non-fatally.
pub fn ensure_downloaded(path: &Path) -> bool {
    let Ok(c_path) = CString::new(path.to_string_lossy().as_bytes()) else {
        return false;
    };
    // SAFETY: `c_path` is a valid NUL-terminated string that outlives the call;
    // the Swift side only reads it and returns a bool.
    unsafe { medme_icloud_ensure_downloaded(c_path.as_ptr()) }
}

/// Number of `POLL_INTERVAL` waits after triggering a download before giving up.
/// The iCloud download is asynchronous, so a single immediate retry can't see
/// the bytes; we poll for a short bounded window (≈2s total) instead — enough
/// for a small already-uploaded object on a live connection, without hanging
/// the UI if it's genuinely unavailable.
const DOWNLOAD_POLLS: u32 = 20;
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// Read a CAS object that may be evicted (a dataless iCloud placeholder) on
/// this device. Fast path: read + verify the sha256 (objects are
/// content-addressed, so we can prove we got the real bytes). If that fails —
/// file missing/placeholder, or a truncated placeholder read — ask iCloud to
/// download it and poll briefly for the bytes to materialize, re-verifying each
/// time. Best-effort and non-fatal: if it never materializes, the final read's
/// error (or a sha-mismatch error) is returned to the caller, which surfaces it
/// as a command error rather than crashing.
pub fn read_object_ensuring_download(path: &Path, expected_sha: &str) -> std::io::Result<Vec<u8>> {
    if let Some(bytes) = try_read_verified(path, expected_sha) {
        return Ok(bytes);
    }
    // Evicted / missing: trigger a download, then poll for it to land.
    ensure_downloaded(path);
    for _ in 0..DOWNLOAD_POLLS {
        std::thread::sleep(POLL_INTERVAL);
        if let Some(bytes) = try_read_verified(path, expected_sha) {
            return Ok(bytes);
        }
    }
    // Still not materialized: surface a concrete read (propagating a NotFound /
    // IO error), and reject a content mismatch explicitly.
    let bytes = std::fs::read(path)?;
    if core_model::cas::sha256_hex(&bytes) != expected_sha {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "iCloud 对象尚未下载完成或内容校验失败,请联网后重试",
        ));
    }
    Ok(bytes)
}

/// Read `path` and return its bytes only if their sha256 matches `expected_sha`
/// (an evicted placeholder either won't read or won't match). `None` on any
/// read error or hash mismatch, so the caller can trigger a download + retry.
fn try_read_verified(path: &Path, expected_sha: &str) -> Option<Vec<u8>> {
    let bytes = std::fs::read(path).ok()?;
    if core_model::cas::sha256_hex(&bytes) == expected_sha {
        Some(bytes)
    } else {
        None
    }
}
