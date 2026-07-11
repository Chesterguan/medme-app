//! End-to-end round trip for the isolated DICOM decode subprocess (GHSA-24px).
//!
//! Custom harness (`harness = false`) so THIS test binary can play both roles:
//! when re-invoked with `--decode-dicom …` by the parent wrapper it runs the
//! decode child; otherwise it runs the assertions, which call the real parent
//! functions (`render_png` / `decode_frame_ipc`). Those spawn `current_exe()`
//! (this binary) with the hidden flag, so we exercise the genuine stdin→decode
//! →stdout path — the same code `commands::render_dicom` uses in production —
//! without having to build the full Tauri binary.
//!
//! A normal `libtest` harness can't do this: it would try to parse
//! `--decode-dicom` as a test filter and abort before our child code runs.

use std::path::Path;

fn sample(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../examples/demo-dataset/dicom")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn main() {
    // Child role: this same binary, re-spawned by the parent wrapper.
    let argv: Vec<String> = std::env::args().collect();
    if let Some(pos) = argv
        .iter()
        .position(|a| a == desktop_lib::dicom_subprocess::DECODE_FLAG)
    {
        std::process::exit(desktop_lib::dicom_subprocess::run_child(&argv[pos + 1..]));
    }

    // Parent/test role.
    valid_dicom_renders_to_png();
    valid_frame_decodes_to_ipc_bytes();
    child_decode_failure_degrades();
    bogus_bytes_degrade();
    println!("dicom_subprocess_roundtrip: all checks passed");
}

/// A valid uncompressed DICOM round-trips to a real PNG through the child.
fn valid_dicom_renders_to_png() {
    let png = desktop_lib::dicom_subprocess::render_png(&sample("CT_small.dcm"))
        .expect("valid DICOM should render via the subprocess");
    assert_eq!(
        &png[..8],
        b"\x89PNG\r\n\x1a\n",
        "child stdout must be a real PNG"
    );
}

/// The frame path round-trips the IPC buffer (header length + JSON + pixels).
fn valid_frame_decodes_to_ipc_bytes() {
    let wire = desktop_lib::dicom_subprocess::decode_frame_ipc(&sample("CT_small.dcm"), 0)
        .expect("valid frame should decode via the subprocess");
    let hlen = u32::from_le_bytes(wire[0..4].try_into().unwrap()) as usize;
    assert!(
        wire.len() > 4 + hlen,
        "IPC buffer must carry pixels after its header"
    );
}

/// A valid header but an out-of-range frame index passes the parent's
/// pre-spawn bounds check, reaches the child, and makes it exit non-zero — the
/// parent must degrade (this is the "codec crash confined to child" path).
fn child_decode_failure_degrades() {
    let err = desktop_lib::dicom_subprocess::decode_frame_ipc(&sample("CT_small.dcm"), 9999)
        .expect_err("an undecodable request must degrade, not succeed");
    assert!(
        err.contains("已隔离"),
        "child non-zero exit should degrade, got: {err}"
    );
}

/// Non-DICOM bytes are rejected up front by the parent's header guard (they
/// never even reach the child) — still a graceful degrade.
fn bogus_bytes_degrade() {
    let err = desktop_lib::dicom_subprocess::render_png(b"definitely not a dicom file")
        .expect_err("bogus input must degrade, not succeed");
    assert!(!err.is_empty(), "degrade must carry an error message");
}
