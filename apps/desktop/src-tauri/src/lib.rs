mod commands;
mod dto;
mod inbox;
mod vault_loc;

use commands::AppState;
use core_model::Vault;
use std::sync::Mutex;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // Open the vault at the user-chosen location (persisted in
            // <app_data_dir>/vault_location), defaulting to <app_data_dir>/vault.
            // Pointing this at a cloud-synced folder is what enables multi-device
            // sync — see vault_loc.rs and commands::set_vault_path.
            let vault_dir = vault_loc::read_vault_location(app.handle());
            std::fs::create_dir_all(&vault_dir).ok();
            let vault = Vault::open(&vault_dir).expect("open vault");
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
