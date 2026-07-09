mod commands;
mod dto;
/// Apple Vision OCR bridge — iOS only (see module docs). Compiled out on the
/// macOS host build and desktop, which keep the oar-ocr pipeline path.
#[cfg(target_os = "ios")]
mod vision;

use commands::AppState;
use core_model::Vault;
use std::sync::Mutex;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // 保险箱放在 iOS 沙盒的 Documents 目录。
            // TODO iCloud container:v1.1 迁移到 iCloud container，实现与桌面经
            // 用户自己的云盘自动同步(见 docs/011_Storage_Sync.md)。现在先用
            // Documents,保证 M1 能开箱可用。
            let docs = app
                .path()
                .document_dir()
                .expect("iOS documents dir");
            std::fs::create_dir_all(&docs).ok();
            let vault_dir = docs.join("vault");
            let vault = Vault::open(&vault_dir).expect("open vault");
            app.manage(AppState {
                vault: Mutex::new(vault),
                vault_dir,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::load_archive,
            commands::ingest_file,
            commands::ingest_bytes,
            commands::get_document,
            commands::read_source_bytes,
            commands::get_patient_profile,
            commands::create_share,
            commands::load_demo_data,
            commands::get_vault_path,
            commands::reset_vault,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
