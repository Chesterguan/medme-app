mod commands;
mod dto;

use commands::AppState;
use core_model::Vault;
use std::sync::Mutex;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_data_dir().expect("app data dir");
            std::fs::create_dir_all(&dir).ok();
            let vault = Vault::open(&dir.join("vault")).expect("open vault");
            app.manage(AppState {
                vault: Mutex::new(vault),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_timeline,
            commands::list_timeline_grouped,
            commands::search,
            commands::get_document,
            commands::import_paths,
            commands::read_source_bytes,
            commands::render_dicom,
            commands::export_vault,
            commands::get_patient_profile,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
