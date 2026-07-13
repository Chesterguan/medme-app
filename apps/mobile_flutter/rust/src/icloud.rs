//! iCloud ubiquity-container 桥(iOS 专用)。
//!
//! 保险箱「真相」(`objects/` + `log/`)可存进本 App 的 iCloud ubiquity 容器,
//! 在用户的苹果设备间同步;`medme.db` 留在沙盒本地、从日志重建(见
//! `core_model::Vault::open_split` 与 `docs/011_Storage_Sync.md`)。
//!
//! ## Swift↔Rust 桥
//! `ios/Runner/AppDelegate.swift` 里用 `@_cdecl` 暴露三个 C-ABI 符号。Runner
//! target 编译该 Swift、链接 cargokit 产的 Rust 静态库,故这些 `extern "C"` 在同一
//! 二进制里直接解析到 Swift 符号——无需插件/CocoaPods/SPM(与旧 Tauri 版机制一致)。
//!
//! 非 iOS 平台(安卓):没有 iCloud,`container_path` 恒为 `None`、`ensure_downloaded`
//! 恒为 no-op,让本模块与调用方在所有平台都能编译,行为上回退到纯本地保险箱。

#[cfg(target_os = "ios")]
use std::ffi::CStr;
use std::ffi::CString;
use std::path::{Path, PathBuf};

#[cfg(target_os = "ios")]
extern "C" {
    fn medme_icloud_container_path() -> *mut std::os::raw::c_char;
    fn medme_icloud_ensure_downloaded(path: *const std::os::raw::c_char) -> bool;
    fn medme_icloud_free(ptr: *mut std::os::raw::c_char);
}

/// 本 App 的 iCloud ubiquity 容器路径;iCloud 不可用/未登录/非 iOS 时返回 `None`。
/// 调用方一律把 `None` 当作「iCloud 不可用」并回退本地保险箱——绝不致命。
pub fn container_path() -> Option<PathBuf> {
    #[cfg(target_os = "ios")]
    {
        // SAFETY: 返回值要么是 null,要么是 malloc 的 NUL 结尾 C 字符串;拷进
        // owned String 后用配套的 medme_icloud_free 释放恰好一次,之后不再使用指针。
        unsafe {
            let ptr = medme_icloud_container_path();
            if ptr.is_null() {
                return None;
            }
            let s = CStr::from_ptr(ptr).to_string_lossy().into_owned();
            medme_icloud_free(ptr);
            if s.is_empty() {
                None
            } else {
                Some(PathBuf::from(s))
            }
        }
    }
    #[cfg(not(target_os = "ios"))]
    {
        None
    }
}

/// 尽力请求 iCloud 下载一个被逐出(dataless 占位)的文件,好让随后读取能看到字节。
/// 出错(路径含 NUL / OS 拒绝 / 非 iOS)返回 false;调用方据此当「没能触发下载」继续,非致命。
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub fn ensure_downloaded(path: &Path) -> bool {
    let Ok(_c_path) = CString::new(path.to_string_lossy().as_bytes()) else {
        return false;
    };
    #[cfg(target_os = "ios")]
    {
        // SAFETY: `_c_path` 是有效的 NUL 结尾字符串,存活到调用结束;Swift 侧只读它、返回 bool。
        unsafe { medme_icloud_ensure_downloaded(_c_path.as_ptr()) }
    }
    #[cfg(not(target_os = "ios"))]
    {
        false
    }
}

/// 触发下载后轮询的次数(iCloud 下载是异步的,立即重试看不到字节,故短暂有界轮询
/// ≈2s——够一个已上传的小对象在联网时落地,又不至于卡住 UI)。
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
const DOWNLOAD_POLLS: u32 = 20;
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// 读取一个可能被逐出(dataless iCloud 占位)的 CAS 对象。快路径:读 + 校验 sha256
/// (内容寻址,能证明拿到真字节)。失败(缺失/占位/截断)则请求 iCloud 下载并短暂
/// 轮询等字节落地,每次重校验。尽力而为、非致命:始终没落地则返回最终读取的错误
/// (或 sha 不匹配错误),由调用方作为命令错误上抛而非崩溃。
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub fn read_object_ensuring_download(path: &Path, expected_sha: &str) -> std::io::Result<Vec<u8>> {
    if let Some(bytes) = try_read_verified(path, expected_sha) {
        return Ok(bytes);
    }
    ensure_downloaded(path);
    for _ in 0..DOWNLOAD_POLLS {
        std::thread::sleep(POLL_INTERVAL);
        if let Some(bytes) = try_read_verified(path, expected_sha) {
            return Ok(bytes);
        }
    }
    let bytes = std::fs::read(path)?;
    if core_model::cas::sha256_hex(&bytes) != expected_sha {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "iCloud 对象尚未下载完成或内容校验失败,请联网后重试",
        ));
    }
    Ok(bytes)
}

#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
fn try_read_verified(path: &Path, expected_sha: &str) -> Option<Vec<u8>> {
    let bytes = std::fs::read(path).ok()?;
    if core_model::cas::sha256_hex(&bytes) == expected_sha {
        Some(bytes)
    } else {
        None
    }
}
