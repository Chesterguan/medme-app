//! Watch Folder(自动收件箱):监听一个用户可见目录,文件一出现就自动走
//! `pipeline::ingest`(与手动拖拽导入同一条路径),成功后移入 `已导入/` 子目录,
//! 并发出 `vault-changed` 事件让前端刷新时间线/病人 banner。
//!
//! 见 docs/011_Storage_Sync.md §7。

use crate::commands::AppState;
use notify::Watcher;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

const IMPORTED_DIR_NAME: &str = "已导入";
const CONFIG_FILE_NAME: &str = "config.json";
const DEBOUNCE: Duration = Duration::from_millis(1000);

#[derive(Serialize, Deserialize, Default)]
struct Config {
    inbox: Option<String>,
}

/// 默认收件箱:`~/Documents/MedMe收件箱`——用户可见,方便手机云同步(iCloud Drive /
/// OneDrive 等)直接指向这里。
fn default_inbox_path(app: &AppHandle) -> PathBuf {
    let base = app
        .path()
        .document_dir()
        .unwrap_or_else(|_| std::env::temp_dir());
    base.join("MedMe收件箱")
}

fn config_path(app: &AppHandle) -> PathBuf {
    let dir = app.path().app_data_dir().expect("app data dir");
    dir.join(CONFIG_FILE_NAME)
}

/// 读取收件箱路径;配置缺失/损坏则回落到默认值(不落盘,由调用方决定是否写入)。
pub fn read_inbox_path(app: &AppHandle) -> PathBuf {
    let cfg_path = config_path(app);
    if let Ok(bytes) = fs::read(&cfg_path) {
        if let Ok(cfg) = serde_json::from_slice::<Config>(&bytes) {
            if let Some(inbox) = cfg.inbox.filter(|s| !s.trim().is_empty()) {
                return PathBuf::from(inbox);
            }
        }
    }
    default_inbox_path(app)
}

/// 把收件箱路径写入 `<app_data_dir>/config.json`。
pub fn write_inbox_path(app: &AppHandle, path: &Path) -> std::io::Result<()> {
    let cfg_path = config_path(app);
    if let Some(parent) = cfg_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let cfg = Config {
        inbox: Some(path.to_string_lossy().to_string()),
    };
    let bytes = serde_json::to_vec_pretty(&cfg)?;
    fs::write(cfg_path, bytes)
}

fn imported_dir(inbox: &Path) -> PathBuf {
    inbox.join(IMPORTED_DIR_NAME)
}

/// 列出收件箱内可导入的常规文件:跳过 `已导入/` 子目录(及其他子目录)、隐藏/点文件、
/// `.tmp`/半成品文件。
fn importable_files(inbox: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let entries = match fs::read_dir(inbox) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue; // 跳过目录(含 已导入/)、符号链接目录等
        }
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue; // 隐藏/点文件
        }
        let lower = name.to_lowercase();
        if lower.ends_with(".tmp") || lower.ends_with(".part") || lower.ends_with(".crdownload") {
            continue; // 半成品/临时下载文件
        }
        out.push(path);
    }
    out
}

/// 把文件移到 `已导入/`;同名冲突时追加数字后缀(`_1`、`_2`……)。
fn move_to_imported(src: &Path, dest_dir: &Path) -> std::io::Result<()> {
    let file_name = src
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no file name"))?;
    let mut dest = dest_dir.join(file_name);
    if dest.exists() {
        let stem = Path::new(file_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("file");
        let ext = Path::new(file_name).extension().and_then(|s| s.to_str());
        let mut counter = 1u32;
        loop {
            let candidate_name = match ext {
                Some(ext) => format!("{stem}_{counter}.{ext}"),
                None => format!("{stem}_{counter}"),
            };
            let candidate = dest_dir.join(candidate_name);
            if !candidate.exists() {
                dest = candidate;
                break;
            }
            counter += 1;
        }
    }
    fs::rename(src, dest)
}

/// 扫描收件箱:对每个候选文件走 `pipeline::ingest`(与手动导入同一路径)。
/// 成功 → 移入 `已导入/`(失败留在原地,下次扫描重试,不致命)。
/// 至少成功导入一个 → 重建 encounter 分组 + 发出 `vault-changed` 事件供前端刷新。
pub fn scan_inbox(app: &AppHandle, state: &AppState) {
    let inbox = read_inbox_path(app);
    if fs::create_dir_all(&inbox).is_err() {
        return;
    }

    let files = importable_files(&inbox);
    if files.is_empty() {
        return;
    }
    let dest_dir = imported_dir(&inbox);

    let mut imported = 0usize;
    for path in files {
        let ingest_result = {
            let vault = match state.vault.lock() {
                Ok(v) => v,
                Err(_) => {
                    eprintln!("[inbox] vault lock poisoned, abort scan");
                    return;
                }
            };
            // 与手动导入同一条隔离路径:catch_unwind 把解析栈里的 panic 变成 Err,
            // 绝不让它穿过持有的 Vault 锁去毒化互斥量 / 打死监听线程。
            crate::commands::ingest_guarded(&vault, &path)
        };
        match ingest_result {
            Ok(_) => {
                if let Err(e) = fs::create_dir_all(&dest_dir) {
                    eprintln!("[inbox] cannot create {}: {e}", dest_dir.display());
                    continue; // 留在原地,不算导入失败,只是没挪走
                }
                match move_to_imported(&path, &dest_dir) {
                    Ok(()) => imported += 1,
                    Err(e) => {
                        eprintln!(
                            "[inbox] imported but failed to move {}: {e}",
                            path.display()
                        );
                    }
                }
            }
            Err(e) => {
                eprintln!("[inbox] ingest failed for {}: {e}", path.display());
                // 留在原地(不移动),下次扫描重试
            }
        }
    }

    if imported > 0 {
        if let Ok(vault) = state.vault.lock() {
            let _ = vault.rebuild_encounters(); // 与手动导入路径一致(幂等)
        }
        let _ = app.emit("vault-changed", ());
    }
}

/// 确保收件箱目录存在、做一次启动扫描(补上应用未运行期间落地的文件)、然后开始监听
/// (非递归;事件到达后防抖 ~1s 再整体重扫——比逐文件处理更简单也更抗半写入/重复事件)。
///
/// 返回的 watcher 需要被调用方保存(存进 `AppState`),否则一超出作用域就会被 drop 从而
/// 停止监听。
pub fn start(app: &AppHandle) -> notify::Result<notify::RecommendedWatcher> {
    let inbox = read_inbox_path(app);
    fs::create_dir_all(&inbox).ok();

    {
        let state = app.state::<AppState>();
        scan_inbox(app, &state);
    }

    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = tx.send(()); // 接收端已 drop(应用关闭)时静默忽略
        }
    })?;
    watcher.watch(&inbox, notify::RecursiveMode::NonRecursive)?;

    let app_for_thread = app.clone();
    std::thread::spawn(move || {
        loop {
            // 阻塞等第一个事件;watcher 被 drop(应用退出)→ 通道断开 → 线程退出
            if rx.recv().is_err() {
                break;
            }
            // 防抖:持续吸收后续事件,直到安静 DEBOUNCE 时长
            loop {
                match rx.recv_timeout(DEBOUNCE) {
                    Ok(()) => continue,
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            // 应用状态在整个生命周期内都已 managed,这里取用是安全的
            let state = app_for_thread.state::<AppState>();
            scan_inbox(&app_for_thread, &state);
        }
    });

    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn importable_files_skips_imported_dir_hidden_and_partial_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        std::fs::write(root.join("report.pdf"), b"x").unwrap();
        std::fs::write(root.join(".DS_Store"), b"x").unwrap();
        std::fs::write(root.join("downloading.tmp"), b"x").unwrap();
        std::fs::write(root.join("partial.part"), b"x").unwrap();
        std::fs::write(root.join("partial.crdownload"), b"x").unwrap();
        std::fs::create_dir_all(root.join(IMPORTED_DIR_NAME)).unwrap();
        std::fs::write(root.join(IMPORTED_DIR_NAME).join("old.pdf"), b"x").unwrap();
        std::fs::create_dir_all(root.join("some_subdir")).unwrap();

        let mut names: Vec<String> = importable_files(root)
            .into_iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
            .collect();
        names.sort();

        assert_eq!(names, vec!["report.pdf".to_string()]);
    }

    #[test]
    fn move_to_imported_dedupes_on_name_clash() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = dir.path();
        let dest_dir = inbox.join(IMPORTED_DIR_NAME);
        std::fs::create_dir_all(&dest_dir).unwrap();
        std::fs::write(dest_dir.join("report.pdf"), b"old").unwrap();

        let src = inbox.join("report.pdf");
        std::fs::write(&src, b"new").unwrap();

        move_to_imported(&src, &dest_dir).unwrap();

        assert!(dest_dir.join("report.pdf").exists());
        assert!(dest_dir.join("report_1.pdf").exists());
        assert_eq!(
            std::fs::read_to_string(dest_dir.join("report_1.pdf")).unwrap(),
            "new"
        );
    }
}
