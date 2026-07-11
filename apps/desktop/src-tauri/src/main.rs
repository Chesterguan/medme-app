// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Hidden decode-child subcommand (advisory GHSA-24px): when spawned as
    // `medme-desktop --decode-dicom <mode> [frame_index]`, this process is the
    // isolated DICOM pixel decoder — read DICOM from stdin, decode via the C/C++
    // codecs, write the result to stdout, exit. It must be handled BEFORE the
    // normal Tauri run so the child never opens the vault or a window. A codec
    // memory-corruption exploit is thus confined to this ephemeral child; the
    // parent (see `commands::render_dicom` / `decode_dicom_frame`) treats a
    // crash/timeout as an "unable to render" degrade.
    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args
        .iter()
        .position(|a| a == desktop_lib::dicom_subprocess::DECODE_FLAG)
    {
        std::process::exit(desktop_lib::dicom_subprocess::run_child(&args[pos + 1..]));
    }

    desktop_lib::run()
}
