//! Move or adopt this vault into a new root directory, so the user can point
//! the vault at a cloud-synced folder (iCloud Drive / 坚果云) for serverless
//! multi-device sync (see `docs/011_Storage_Sync.md`).
//!
//! Two cases, decided by whether `new_root` already holds a vault (any `.jsonl`
//! log segment):
//! - **new_root has NO vault** → **MOVE**: relocate `objects/`, `log/`,
//!   `medme.db`, `VERSION` into it. Rename on the same filesystem (fast, atomic
//!   per entry); across filesystems, copy + verify + remove.
//! - **new_root ALREADY has a vault** (another device populated the shared
//!   folder) → **ADOPT + MERGE**: copy this vault's log segments and CAS
//!   objects in. Segment filenames are per-device (never collide across
//!   devices) and CAS objects are content-addressed (dedup by identical path).
//!   The target's derived `medme.db` is left in place to be rebuilt from the
//!   merged log — we do NOT copy this device's `medme.db` over the target's.
//!
//! Data-safety invariant: the source is never partially destroyed before the
//! target is complete. Cross-filesystem moves copy + verify every item, then
//! remove the source; same-filesystem renames are atomic per entry. Adopt only
//! ever copies — it never touches the source.

use crate::{MedmeError, Vault};
use std::path::Path;

/// errno for a cross-device link: `rename(2)` cannot move across filesystems,
/// so we fall back to copy + verify + remove when we see it.
const EXDEV: i32 = 18;

/// How to treat a destination file that already exists during a copy.
#[derive(Clone, Copy)]
enum OnExisting {
    /// Overwrite it (used for a plain move into a fresh target).
    Overwrite,
    /// Skip it — the content is immutable and addressed by its path (CAS).
    SkipExisting,
    /// Keep whichever file is longer — an append-only log segment is a prefix
    /// superset, so the larger one is authoritative and never truncates data.
    KeepLarger,
}

impl Vault {
    /// Make `new_root` hold this vault's combined data (see the module docs for
    /// the move-vs-adopt decision). Does not mutate `self`'s in-memory state;
    /// callers reopen `Vault::open(new_root)` afterwards.
    pub fn relocate_to(&self, new_root: &Path) -> Result<(), MedmeError> {
        let src = self.root().to_path_buf();
        std::fs::create_dir_all(new_root)?;

        if paths_equal(&src, new_root) {
            return Err(MedmeError::Other("新位置就是当前位置,无需更换".to_string()));
        }
        if is_descendant(new_root, &src) {
            return Err(MedmeError::Other(
                "新位置不能位于当前保险箱目录内部".to_string(),
            ));
        }

        if target_has_vault(new_root) {
            adopt_into(&src, new_root)
        } else {
            move_into(&src, new_root)
        }
    }
}

/// True if `new_root/log` contains at least one `.jsonl` segment — i.e. another
/// device has already populated this folder as a vault.
fn target_has_vault(new_root: &Path) -> bool {
    let log_dir = new_root.join("log");
    let Ok(entries) = std::fs::read_dir(&log_dir) else {
        return false;
    };
    entries
        .filter_map(|e| e.ok())
        .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("jsonl"))
}

/// MOVE: relocate the four vault entries into a target with no existing vault.
fn move_into(src: &Path, new_root: &Path) -> Result<(), MedmeError> {
    for item in ["objects", "log", "medme.db", "VERSION"] {
        let from = src.join(item);
        if !from.exists() {
            continue;
        }
        move_path(&from, &new_root.join(item))?;
    }
    Ok(())
}

/// ADOPT + MERGE: copy this vault's log segments and CAS objects into a target
/// that already holds a vault. Never removes anything from the source, and
/// never copies this device's `medme.db` — the target's derived DB is rebuilt.
fn adopt_into(src: &Path, new_root: &Path) -> Result<(), MedmeError> {
    // CAS objects: content-addressed, immutable → copy any the target lacks.
    let src_objects = src.join("objects");
    if src_objects.is_dir() {
        copy_tree(
            &src_objects,
            &new_root.join("objects"),
            OnExisting::SkipExisting,
        )?;
    }

    // Log segments: copy every `.jsonl` segment (this device's plus any legacy
    // or peer segments already present in the source). Per-device names don't
    // collide across devices; if a same-named segment exists, keep the larger
    // (append-only superset) so we never truncate a more-complete log.
    let src_log = src.join("log");
    let dst_log = new_root.join("log");
    std::fs::create_dir_all(&dst_log)?;
    if let Ok(entries) = std::fs::read_dir(&src_log) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().and_then(|x| x.to_str()) == Some("jsonl") {
                copy_file(
                    &path,
                    &dst_log.join(entry.file_name()),
                    OnExisting::KeepLarger,
                )?;
            }
        }
    }
    Ok(())
}

/// Move one entry (file or directory) `from` → `to`. Prefers an atomic rename;
/// on a cross-filesystem error (or when the target already exists) it copies +
/// verifies, then removes the source — so the source is never destroyed before
/// the target is fully written.
fn move_path(from: &Path, to: &Path) -> Result<(), MedmeError> {
    if !to.exists() {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            // EXDEV: different filesystem, rename impossible → copy fallback.
            Err(e) if e.raw_os_error() == Some(EXDEV) => {}
            Err(e) => return Err(e.into()),
        }
    }
    if from.is_dir() {
        copy_tree(from, to, OnExisting::Overwrite)?;
        std::fs::remove_dir_all(from)?;
    } else {
        copy_file(from, to, OnExisting::Overwrite)?;
        std::fs::remove_file(from)?;
    }
    Ok(())
}

/// Recursively copy `src` dir contents into `dst`, applying `policy` to files
/// that already exist in `dst`.
fn copy_tree(src: &Path, dst: &Path, policy: OnExisting) -> Result<(), MedmeError> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&from, &to, policy)?;
        } else {
            copy_file(&from, &to, policy)?;
        }
    }
    Ok(())
}

/// Copy a single file `from` → `to`, honouring `policy` when `to` exists, and
/// verifying the copied byte count matches the source (guards against a
/// silently truncated copy of medical data).
fn copy_file(from: &Path, to: &Path, policy: OnExisting) -> Result<(), MedmeError> {
    if to.exists() {
        match policy {
            OnExisting::SkipExisting => return Ok(()),
            OnExisting::KeepLarger => {
                let src_len = std::fs::metadata(from)?.len();
                let dst_len = std::fs::metadata(to)?.len();
                if src_len <= dst_len {
                    return Ok(());
                }
            }
            OnExisting::Overwrite => {}
        }
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let copied = std::fs::copy(from, to)?;
    let src_len = std::fs::metadata(from)?.len();
    if copied != src_len {
        return Err(MedmeError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!(
                "copy verification failed for {}: copied {copied} of {src_len} bytes",
                from.display()
            ),
        )));
    }
    Ok(())
}

/// Best-effort canonicalized path equality (falls back to a lexical compare if
/// a path can't be canonicalized).
fn paths_equal(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// True if `child` is `ancestor` itself or nested inside it.
fn is_descendant(child: &Path, ancestor: &Path) -> bool {
    match (child.canonicalize(), ancestor.canonicalize()) {
        (Ok(c), Ok(a)) => c.starts_with(&a),
        _ => child.starts_with(ancestor),
    }
}
