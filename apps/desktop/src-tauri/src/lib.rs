mod commands;
mod dto;
mod inbox;
mod vault_loc;

use commands::AppState;
use core_model::Vault;
use std::path::Path;
use std::sync::Mutex;
use tauri::Manager;

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
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
            let vault = Vault::open_with_device_id(&vault_dir, &device_id).expect("open vault");
            app.manage(AppState {
                vault: Mutex::new(vault),
                inbox_watcher: Mutex::new(None),
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
            commands::import_paths,
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
            commands::write_text_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
