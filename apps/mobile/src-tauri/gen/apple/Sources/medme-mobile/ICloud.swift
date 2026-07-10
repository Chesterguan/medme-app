// iCloud ubiquity-container bridge for MedMe iOS.
//
// The vault's truth (objects/ + log/) can live in the app's iCloud ubiquity
// container so it syncs across the user's Apple devices; medme.db stays local
// in the sandbox and is rebuilt (see core-model `Vault::open_split` and
// `docs/011_Storage_Sync.md`). This file exposes the two OS calls Rust needs,
// as plain C-ABI symbols via `@_cdecl` — mirroring `OcrVision.swift`. The iOS
// app target compiles this Swift file and links Rust's `libapp.a`, so the Rust
// `extern "C"` declarations in `src/icloud.rs` resolve directly against these
// symbols inside the same binary — no Tauri Swift plugin, CocoaPods or SPM.

import Foundation

/// Resolve the app's iCloud ubiquity container and return its POSIX path as a
/// malloc'd, NUL-terminated UTF-8 C string (caller frees via
/// `medme_icloud_free`). Returns null if iCloud is unavailable / the user is
/// not signed in, or the container can't be provisioned.
///
/// `url(forUbiquityContainerIdentifier:)` performs blocking I/O and MUST NOT
/// run on the main thread, so we always hop to a background queue and block for
/// the result via a semaphore (the caller side is a synchronous FFI, like the
/// Vision bridge). Passing `nil` selects the first
/// `com.apple.developer.ubiquity-container-identifiers` entitlement value.
@_cdecl("medme_icloud_container_path")
public func medme_icloud_container_path() -> UnsafeMutablePointer<CChar>? {
  var resolved: String?
  let sem = DispatchSemaphore(value: 0)
  DispatchQueue.global(qos: .userInitiated).async {
    resolved = FileManager.default.url(forUbiquityContainerIdentifier: nil)?.path
    sem.signal()
  }
  sem.wait()
  guard let path = resolved, !path.isEmpty else { return nil }
  return path.withCString { strdup($0) }
}

/// Best-effort trigger a download of an evicted (dataless) ubiquitous item at
/// `pathC` so a subsequent read can see its bytes. Returns true if the download
/// request was accepted (the download itself is asynchronous), false on error.
@_cdecl("medme_icloud_ensure_downloaded")
public func medme_icloud_ensure_downloaded(_ pathC: UnsafePointer<CChar>) -> Bool {
  let url = URL(fileURLWithPath: String(cString: pathC))
  do {
    try FileManager.default.startDownloadingUbiquitousItem(at: url)
    return true
  } catch {
    return false
  }
}

/// Free a buffer returned by `medme_icloud_container_path`. `strdup` allocates
/// with `malloc`, matching the Rust-side free.
@_cdecl("medme_icloud_free")
public func medme_icloud_free(_ ptr: UnsafeMutablePointer<CChar>?) {
  if let ptr = ptr { free(ptr) }
}
