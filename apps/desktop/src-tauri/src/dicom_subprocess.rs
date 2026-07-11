//! Out-of-process DICOM pixel decoding (advisory GHSA-24px).
//!
//! The vendored C/C++ JPEG 2000 (OpenJPEG) / JPEG-LS (CharLS) decoders behind
//! `dicom`'s `codecs` feature are a memory-corruption RCE surface: an
//! attacker-crafted codestream can corrupt memory *inside the C/C++ library*.
//! `catch_unwind` does NOT contain that — it is not a memory-safety boundary for
//! C++ exceptions or segfaults/aborts. So instead of decoding in the main
//! process (which holds the open vault), desktop decodes pixels in a short-lived
//! CHILD process — the same binary re-invoked as `medme-desktop --decode-dicom
//! <mode> [frame_index]`:
//!
//!   parent: check_bounds → spawn child → pipe DICOM to stdin → read result
//!           from stdout, under a timeout + output-size cap
//!   child:  read DICOM from stdin → decode (C/C++ codecs) → write result to
//!           stdout → exit 0; any failure → exit non-zero
//!
//! A memory-corruption exploit (crash, segfault, abort, hang) in the codec is
//! therefore confined to the ephemeral child: it can never touch the vault or
//! the main process. The parent treats a non-zero exit / timeout / oversized
//! output as "unable to render" and degrades exactly like the existing
//! unsupported-transfer-syntax path (the frontend already handles that `Err`).
//!
//! Valid DICOMs are unaffected: the child produces the identical bytes the old
//! in-process `dicom::render_png` / `dicom::decode_frame(...).into_ipc_bytes()`
//! produced, and the parent relays them verbatim.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Hidden argv flag that switches the binary into decode-child mode. `main.rs`
/// checks for this before starting Tauri.
pub const DECODE_FLAG: &str = "--decode-dicom";

/// Child mode: stdout = windowed PNG (`dicom::render_png`).
const MODE_RENDER: &str = "render";
/// Child mode: stdout = one frame's IPC bytes (`dicom::decode_frame` →
/// `into_ipc_bytes`: 4-byte header length + JSON header + raw pixels).
const MODE_FRAME: &str = "frame";

/// Kill the child if it hasn't finished within this budget — a malicious
/// codestream can wedge the C/C++ decoder in a long-running loop.
const CHILD_TIMEOUT: Duration = Duration::from_secs(15);

/// Reject a child whose stdout exceeds this. The header size guard
/// (`dicom::check_bounds`, run in the parent before spawning) already caps
/// decoded pixels at 512 MiB; this is a second backstop on the wire. PNG is
/// compressed and a frame is ~raw pixels + a tiny header, so 768 MiB is ample
/// headroom over any legitimate output.
const MAX_OUTPUT_BYTES: usize = 768 * 1024 * 1024;

/// Reads from `r` until EOF, failing if more than `cap` bytes arrive.
fn read_capped(r: &mut impl Read, cap: usize) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = r.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        if buf.len() + n > cap {
            return Err("DICOM 解码输出超出上限(已隔离)".to_string());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    Ok(buf)
}

/// Feeds `input` to `cmd`'s stdin, reads its stdout (capped at `max_output`),
/// and enforces `timeout` (kill on overrun). Returns the child's stdout on a
/// clean exit-0; every other outcome — spawn failure, non-zero exit, crash,
/// timeout, oversized output — is a plain `Err(String)` the caller degrades on.
///
/// The command's args must already be set by the caller; this owns the stdio
/// wiring. Split out from [`run_parent`] (which hard-codes `current_exe()`) so
/// the timeout/cap/degrade logic is unit-testable against a fake child.
fn spawn_and_pipe(
    mut cmd: Command,
    input: &[u8],
    timeout: Duration,
    max_output: usize,
) -> Result<Vec<u8>, String> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;

    // Feed stdin from a dedicated thread so a full stdout pipe can't deadlock us
    // against a child that is blocked writing while we're blocked writing.
    let mut stdin = child.stdin.take().ok_or("child stdin unavailable")?;
    let owned_input = input.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&owned_input);
        // stdin drops here → child sees EOF on its input.
    });

    // Drain stdout (capped) on another thread, so the main thread is free to
    // enforce the wall-clock timeout via try_wait.
    let mut stdout = child.stdout.take().ok_or("child stdout unavailable")?;
    let reader = std::thread::spawn(move || read_capped(&mut stdout, max_output));

    // Poll for exit up to the deadline; kill (and reap) on timeout.
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("DICOM 解码超时(已隔离)".to_string());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(e.to_string()),
        }
    };

    let _ = writer.join();
    let output = reader
        .join()
        .map_err(|_| "decode reader thread panicked".to_string())??;

    // Non-zero exit = decode failure OR a codec crash/abort on a malicious
    // codestream → degrade like an unsupported transfer syntax.
    if !status.success() {
        return Err("DICOM 解码失败(已隔离)".to_string());
    }
    Ok(output)
}

/// Parent side: spawn the isolated decode child (this binary, re-invoked with
/// [`DECODE_FLAG`] + `mode_args`), feed it `dcm_bytes` on stdin, and return its
/// stdout.
fn run_parent(mode_args: &[&str], dcm_bytes: &[u8]) -> Result<Vec<u8>, String> {
    // Cheap, pure-Rust header guard in the MAIN process: reject decode/
    // decompression bombs before we even spawn the codec child.
    dicom::check_bounds(dcm_bytes).map_err(|e| e.to_string())?;

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.arg(DECODE_FLAG).args(mode_args);
    spawn_and_pipe(cmd, dcm_bytes, CHILD_TIMEOUT, MAX_OUTPUT_BYTES)
}

/// Decode a DICOM to a windowed PNG in an isolated child process. Drop-in
/// replacement for the old in-process `dicom::render_png`.
pub fn render_png(dcm_bytes: &[u8]) -> Result<Vec<u8>, String> {
    run_parent(&[MODE_RENDER], dcm_bytes)
}

/// Decode one DICOM frame to IPC bytes (header + raw pixels) in an isolated
/// child process. Drop-in replacement for the old in-process
/// `dicom::decode_frame(..).into_ipc_bytes()`.
pub fn decode_frame_ipc(dcm_bytes: &[u8], frame_index: u32) -> Result<Vec<u8>, String> {
    run_parent(&[MODE_FRAME, &frame_index.to_string()], dcm_bytes)
}

/// Child side: run one decode from stdin→stdout and return the process exit
/// code. `args` is the argv slice *after* [`DECODE_FLAG`] (mode, then optional
/// frame index). This is where the C/C++ codecs actually run, isolated from the
/// main process. Every failure maps to a non-zero code.
///
///   0 = success · 1 = I/O error · 2 = unknown mode · 3 = decode failure
pub fn run_child(args: &[String]) -> i32 {
    let mut input = Vec::new();
    if std::io::stdin().read_to_end(&mut input).is_err() {
        return 1;
    }

    // `render_png` / `decode_frame` re-run the header size guard internally, so
    // the child is bounds-checked too (defense in depth alongside the parent's
    // pre-spawn `check_bounds`).
    let result = match args.first().map(String::as_str) {
        Some(MODE_RENDER) => dicom::render_png(&input),
        Some(MODE_FRAME) => {
            let idx: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            dicom::decode_frame(&input, idx).and_then(|f| f.into_ipc_bytes())
        }
        _ => return 2,
    };

    match result {
        Ok(bytes) => {
            let mut out = std::io::stdout();
            if out.write_all(&bytes).is_err() || out.flush().is_err() {
                return 1;
            }
            0
        }
        Err(_) => 3,
    }
}

// Unit tests for the parent-side spawn/timeout/cap/degrade wrapper, driven
// against fake children (small `/bin/sh` programs) so we exercise every failure
// branch without needing the real codec child. The real end-to-end round trip
// through `run_child` (stdin DICOM → stdout PNG/IPC) is covered by the
// custom-harness integration test `tests/dicom_subprocess_roundtrip.rs`.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh(script: &str) -> Command {
        let mut c = Command::new("/bin/sh");
        c.arg("-c").arg(script);
        c
    }

    #[test]
    fn relays_child_stdout_on_success() {
        // `cat` echoes our stdin straight back → parent returns it verbatim.
        let out = spawn_and_pipe(sh("cat"), b"round trip", Duration::from_secs(5), 1 << 20)
            .expect("clean exit-0 child");
        assert_eq!(out, b"round trip");
    }

    #[test]
    fn nonzero_exit_degrades() {
        let err = spawn_and_pipe(sh("exit 7"), b"x", Duration::from_secs(5), 1 << 20)
            .expect_err("non-zero exit must degrade");
        assert!(err.contains("已隔离"), "expected degrade error, got: {err}");
    }

    #[test]
    fn timeout_kills_and_degrades() {
        // Child sleeps well past the 150ms budget → parent kills it and degrades.
        let start = Instant::now();
        let err = spawn_and_pipe(sh("sleep 30"), b"x", Duration::from_millis(150), 1 << 20)
            .expect_err("a hung child must time out");
        assert!(err.contains("超时"), "expected a timeout error, got: {err}");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "kill was not prompt"
        );
    }

    #[test]
    fn oversized_output_degrades() {
        // Child floods stdout with far more than the 1 KiB cap → parent rejects.
        let err = spawn_and_pipe(
            sh("head -c 100000 /dev/zero"),
            b"x",
            Duration::from_secs(5),
            1024,
        )
        .expect_err("output over the cap must degrade");
        assert!(
            err.contains("上限"),
            "expected an output-cap error, got: {err}"
        );
    }
}
