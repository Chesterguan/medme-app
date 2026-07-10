mod commands;
mod dto;
/// Apple Vision OCR bridge — iOS only (see module docs). Compiled out on the
/// macOS host build and desktop, which keep the oar-ocr pipeline path.
#[cfg(target_os = "ios")]
mod vision;

use commands::AppState;
use core_model::Vault;
use std::path::Path;
use std::sync::Mutex;
use tauri::Manager;

/// This machine's persistent device id, stored in `<app_data_dir>/device_id`
/// (OUTSIDE the vault, which may live in a shared/synced folder). Created with a
/// fresh random id on first launch. Because it is per-DEVICE, two devices
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
        eprintln!("[device_id] failed to persist device id to {file:?}: {e}");
    }
    id
}

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
            let docs = app.path().document_dir().expect("iOS documents dir");
            std::fs::create_dir_all(&docs).ok();
            let vault_dir = docs.join("vault");
            // Machine-local device id lives in app_data_dir (NOT the vault
            // Documents folder), so a shared/synced vault never leaks one
            // device's id to another — each keeps its own log segment.
            let data_dir = app.path().app_data_dir().expect("app data dir");
            std::fs::create_dir_all(&data_dir).ok();
            let device_id = machine_device_id(&data_dir);
            let vault = Vault::open_with_device_id(&vault_dir, &device_id).expect("open vault");
            app.manage(AppState {
                vault: Mutex::new(vault),
                vault_dir,
            });

            // Android on-device OCR: the PP-OCRv5 models are shipped in the APK
            // and extracted to <dataDir>/oar by MainActivity.onCreate (Kotlin)
            // before this runs. Point the oar-ocr engine at them. On failure OCR
            // is simply unavailable (ingest falls back to StoredNoText) — never
            // fatal to app startup. iOS/desktop don't need this: iOS routes
            // images to Apple Vision and auto-downloads models for scanned PDFs;
            // desktop auto-downloads into ~/.oar.
            #[cfg(target_os = "android")]
            init_android_ocr_models(app);

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::load_archive,
            commands::ingest_file,
            commands::ingest_bytes,
            commands::get_document,
            commands::read_source_bytes,
            commands::read_share_bytes,
            commands::get_patient_profile,
            commands::create_share,
            commands::load_demo_data,
            commands::get_vault_path,
            commands::reset_vault,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Locate the PP-OCRv5 model files extracted from the APK and point the
/// `ocr`/oar-ocr engine at them. The Kotlin `MainActivity.onCreate` copies
/// `assets/oar/*` into `<Context.dataDir>/oar` on launch; `app_data_dir()`
/// resolves to that same `dataDir`, so we look under `<app_data_dir>/oar`.
///
/// Best-effort: if the models aren't present (copy failed / older install),
/// we leave the engine pointed at nothing and log — the first OCR attempt then
/// errors and ingest stores the file without extracted text, exactly as when
/// OCR is unavailable. Never panics.
#[cfg(target_os = "android")]
fn init_android_ocr_models(app: &tauri::App) {
    let model_dir = match app.path().app_data_dir() {
        Ok(dir) => dir.join("oar"),
        Err(e) => {
            eprintln!("[ocr] android: cannot resolve app_data_dir: {e}");
            return;
        }
    };
    let files = [
        "pp-ocrv5_mobile_det.onnx",
        "pp-ocrv5_mobile_rec.onnx",
        "ppocrv5_dict.txt",
    ];
    if files.iter().all(|f| model_dir.join(f).is_file()) {
        ocr::set_model_dir(model_dir.clone());
        eprintln!("[ocr] android: models ready at {}", model_dir.display());
    } else {
        eprintln!(
            "[ocr] android: OCR models missing under {} — OCR disabled until present",
            model_dir.display()
        );
    }
}
