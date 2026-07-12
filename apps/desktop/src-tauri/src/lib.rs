mod commands;
/// Out-of-process DICOM pixel decode (advisory GHSA-24px). `pub` so `main.rs`
/// can dispatch the hidden `--decode-dicom` child subcommand, and the
/// integration test can drive the same round trip.
pub mod dicom_subprocess;
mod dto;
mod inbox;
mod vault_loc;

use commands::AppState;
use core_model::Vault;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{Emitter, Manager};

/// This machine's persistent device id, stored in `<app_data_dir>/device_id`
/// (OUTSIDE the vault, which may live in a shared cloud folder). Created with a
/// fresh random id on first launch. Because it is per-MACHINE, two machines
/// sharing one vault folder each stamp their own log segment
/// (`log/<device_id>-*.jsonl`) → conflict-free sync. See
/// `Vault::open_with_device_id`.
fn machine_device_id(app_data_dir: &Path) -> String {
    let file = app_data_dir.join("device_id");
    if let Ok(s) = std::fs::read_to_string(&file) {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let id = core_model::generate_device_id();
    if let Err(e) = std::fs::write(&file, &id) {
        eprintln!("[device_id] failed to persist machine id to {file:?}: {e}");
    }
    id
}

/// Ingest OS-dropped files (trusted paths from the Tauri core's drag-drop event) off the
/// main thread, then tell the frontend: `import-results` carries the per-file outcomes so
/// the import view can show them, and `vault-changed` refreshes the timeline + banner
/// (same event the watch folder emits). Runs on a worker thread because ingest (OCR /
/// DICOM) must not block the UI event loop that delivered the drop event.
fn handle_file_drop(app: &tauri::AppHandle, paths: Vec<PathBuf>) {
    let app = app.clone();
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let outcomes = {
            // Recover a poisoned lock rather than panicking the worker (see commands::lock).
            let vault = state
                .vault
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let out = commands::ingest_files(&vault, &paths);
            let _ = vault.rebuild_encounters(); // 幂等,与手动导入一致
            out
        };
        let _ = app.emit("import-results", outcomes);
        let _ = app.emit("vault-changed", ());
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        // SECURITY (GHSA-gmg4): handle file drag-drop in the Rust core, where the paths
        // are delivered by the OS and are trustworthy, instead of round-tripping them
        // through the (potentially XSS'd) webview via an `import_paths(paths)` command.
        // The webview can no longer name an arbitrary path to be read into the vault.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
                handle_file_drop(window.app_handle(), paths.clone());
            }
        })
        .setup(|app| {
            // Machine-local device id lives in app_data_dir (NOT the vault), so a
            // shared/cloud vault folder never leaks one machine's id to another —
            // each machine keeps its own per-device log segment.
            let app_dir = app.path().app_data_dir().expect("app data dir");
            let device_id = machine_device_id(&app_dir);
            // Vault root = user-chosen location (persisted in <app_data_dir>/vault_location,
            // default <app_data_dir>/vault). Pointing it at a cloud-synced folder is what
            // enables multi-device sync — see vault_loc.rs + commands::set_vault_path.
            let vault_dir = vault_loc::read_vault_location(app.handle());
            std::fs::create_dir_all(&vault_dir).ok();
            // Open resiliently: the SQLite db is a disposable cache rebuildable
            // from the append-only log, so a corrupt/half-synced `medme.db`
            // (likely when the vault sits in a cloud-synced folder) is wiped and
            // rebuilt instead of `.expect()`-panicking the app into an
            // unopenable state. Only an unreadable TRUTH (log/objects) reaches
            // the error arm — then show a human message instead of a bare crash.
            let vault = match Vault::open_with_device_id_resilient(&vault_dir, &device_id) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[vault] unrecoverable open failure at {vault_dir:?}: {e}");
                    use tauri_plugin_dialog::DialogExt;
                    app.dialog()
                        .message(format!(
                            "无法打开你的健康档案。\n\n位置:{}\n\n数据文件可能损坏或暂时无法访问。若它放在云盘文件夹里,请等云盘同步完成后再打开 MedMe;若问题持续,请联系支持。",
                            vault_dir.display()
                        ))
                        .title("MedMe 暂时无法启动")
                        .blocking_show();
                    std::process::exit(1);
                }
            };
            app.manage(AppState {
                vault: Mutex::new(vault),
                device_id,
                inbox_watcher: Mutex::new(None),
                openable_paths: Mutex::new(HashSet::new()),
            });

            // Watch Folder(见 docs/011_Storage_Sync.md §7):确保收件箱目录存在、启动扫描
            // 一次(补上应用未运行期间落地的文件),再开始监听后续变动。
            let handle = app.handle().clone();
            match inbox::start(&handle) {
                Ok(watcher) => {
                    let state = app.state::<AppState>();
                    *state.inbox_watcher.lock().expect("inbox_watcher lock") = Some(watcher);
                }
                Err(e) => {
                    eprintln!("[inbox] failed to start watch folder: {e}");
                }
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_timeline_grouped,
            commands::search,
            commands::get_document,
            commands::import_via_dialog,
            commands::load_demo_data,
            commands::read_source_bytes,
            commands::render_dicom,
            commands::decode_dicom_frame,
            commands::get_imaging_instances,
            commands::export_vault,
            commands::export_timeline_html,
            commands::create_share,
            commands::get_patient_profile,
            commands::get_inbox_path,
            commands::set_inbox_path,
            commands::open_inbox,
            commands::open_path,
            commands::open_url,
            commands::get_vault_path,
            commands::set_vault_path,
            commands::get_audit_log,
            commands::export_audit_csv,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
