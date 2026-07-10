//! Persisted vault location: which directory `Vault::open` should open at
//! startup. Stored as a single absolute path in `<app_data_dir>/vault_location`
//! (empty/absent → the default `<app_data_dir>/vault`). Letting the user point
//! this at an iCloud Drive / 坚果云 synced folder is what enables serverless
//! multi-device sync (see `set_vault_path` in `commands.rs`).

use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

const LOCATION_FILE: &str = "vault_location";

fn app_data_dir(app: &AppHandle) -> PathBuf {
    app.path().app_data_dir().expect("app data dir")
}

/// The default vault directory: `<app_data_dir>/vault`.
pub fn default_vault_dir(app: &AppHandle) -> PathBuf {
    app_data_dir(app).join("vault")
}

/// The directory the vault should open at. Reads the persisted absolute path;
/// falls back to the default when the file is missing, empty, or unreadable.
pub fn read_vault_location(app: &AppHandle) -> PathBuf {
    let file = app_data_dir(app).join(LOCATION_FILE);
    if let Ok(s) = std::fs::read_to_string(&file) {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    default_vault_dir(app)
}

/// Persist the chosen vault directory (absolute path) so the next launch opens
/// it. Creates `<app_data_dir>` if needed.
pub fn write_vault_location(app: &AppHandle, path: &Path) -> std::io::Result<()> {
    let dir = app_data_dir(app);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(LOCATION_FILE), path.to_string_lossy().as_bytes())
}
