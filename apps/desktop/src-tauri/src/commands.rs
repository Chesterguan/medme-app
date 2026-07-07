use crate::dto::*;
use core_model::Vault;
use std::sync::Mutex;
use tauri::State;
use tauri_plugin_opener::OpenerExt;

pub struct AppState {
    pub vault: Mutex<Vault>,
    /// 收件箱 notify 监听器,需要在 AppState 里存活,否则一超出作用域就会被 drop 从而
    /// 停止监听。setup() 里启动后写入;生命周期与 App 一致。
    pub inbox_watcher: Mutex<Option<notify::RecommendedWatcher>>,
}

fn lock<'a>(s: &'a State<'a, AppState>) -> Result<std::sync::MutexGuard<'a, Vault>, String> {
    s.vault.lock().map_err(|_| "vault lock poisoned".to_string())
}

#[tauri::command]
pub fn list_timeline_grouped(state: State<AppState>) -> Result<Vec<TimelineGroup>, String> {
    let v = lock(&state)?;
    v.rebuild_encounters().map_err(|e| e.to_string())?; // 幂等,确保 CLI 导入的数据也分组
    let mut groups: Vec<(Option<String>, TimelineGroup)> = Vec::new(); // (sort_date, group)
    for (enc, docs) in v.encounters_with_docs().map_err(|e| e.to_string())? {
        let sort = enc.start_date.map(|d| d.to_rfc3339());
        let summary = EncounterSummary::from_encounter(&enc, docs.len() as i64);
        let doc_dtos = docs.iter().map(DocumentSummary::from).collect();
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
        groups.push((sort, TimelineGroup::Document { doc: DocumentSummary::from(&d) }));
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
        document: DocumentSummary::from(&doc),
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
        // 单个文件失败不该拖垮整批 —— 记一条失败结果继续处理剩下的文件(与
        // inbox.rs::scan_inbox 的容错方式一致),而不是 `?` 提前返回丢弃已成功的导入。
        match pipeline::ingest(&v, path) {
            Ok(o) => {
                let status = match o.status {
                    pipeline::IngestStatus::New => "new",
                    pipeline::IngestStatus::Backfilled => "backfilled",
                    pipeline::IngestStatus::Deduped => "deduped",
                    pipeline::IngestStatus::StoredNoText => "stored_no_text",
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

#[tauri::command]
pub fn render_dicom(state: State<AppState>, id: i64) -> Result<tauri::ipc::Response, String> {
    let v = lock(&state)?;
    let sf = v
        .source_file_by_id(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("source_file {id} not found"))?;
    let bytes = std::fs::read(v.root_join(&sf.storage_path)).map_err(|e| e.to_string())?;
    let png = dicom::render_png(&bytes).map_err(|e| e.to_string())?;
    Ok(tauri::ipc::Response::new(png))
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
/// `crate::export::build_timeline_html`)。可在任意浏览器打开、原生渲染中文,
/// 并通过浏览器「打印 / 另存为 PDF」交给医生。
#[tauri::command]
pub fn export_timeline_html(
    state: State<AppState>,
    dest_path: String,
) -> Result<ExportSummary, String> {
    let v = lock(&state)?;
    let (html, record_count) = crate::export::build_timeline_html(&v)?;
    let byte_size = html.len() as i64;
    let sha256 = core_model::cas::sha256_hex(html.as_bytes());
    std::fs::write(&dest_path, html).map_err(|e| e.to_string())?;
    // 审计追踪:导出落盘成功后记入不可变事件日志(见 core-model::audit)。
    v.record_export("timeline_html", &sha256, record_count)
        .map_err(|e| e.to_string())?;
    Ok(ExportSummary {
        file_count: record_count,
        byte_size,
    })
}

/// 端到端加密分享:把全部病历打包成一份自包含加密 HTML 写到 `dest_path`
/// (见 `crate::share::build_encrypted_share`),返回口令(需另行单独告知医生)、
/// 记录数与文件字节数。默认有效期 5 天。
#[tauri::command]
pub fn create_share(
    state: State<AppState>,
    dest_path: String,
    expires_days: Option<u32>,
) -> Result<ShareResult, String> {
    let v = lock(&state)?;
    let days = expires_days.unwrap_or(5);
    let (html, passphrase, record_count) = crate::share::build_encrypted_share(&v, days)?;
    let byte_size = html.len() as i64;
    let sha256 = core_model::cas::sha256_hex(html.as_bytes());
    std::fs::write(&dest_path, html).map_err(|e| e.to_string())?;
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
        name: p.name, gender: p.gender, birth_date: p.birth_date, age: p.age, record_count: p.record_count,
    })
}

/// 收件箱(Watch Folder)当前路径。
#[tauri::command]
pub fn get_inbox_path(app: tauri::AppHandle) -> String {
    crate::inbox::read_inbox_path(&app).to_string_lossy().to_string()
}

/// 修改收件箱路径:持久化到 config.json、创建目录、立即重扫一次。
/// 注意:不会重新定位正在运行的 notify watcher(仍监听旧目录),需重启应用才会
/// 切到新目录监听;新路径下一次启动扫描/手动导入始终立即生效。
#[tauri::command]
pub fn set_inbox_path(app: tauri::AppHandle, state: State<AppState>, path: String) -> Result<(), String> {
    let new_path = std::path::PathBuf::from(&path);
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
    app.opener().open_path(path, None::<String>).map_err(|e| e.to_string())
}

/// 在系统默认浏览器打开一个外部 URL(用于「关于」页的项目主页/源码链接)。
#[tauri::command]
pub fn open_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
    app.opener().open_url(url, None::<String>).map_err(|e| e.to_string())
}

/// 数据保险箱(vault)根目录路径 —— 设置页展示,供用户把它放进 iCloud/坚果云
/// 等云同步目录,实现无需服务器的多设备同步。只读展示,运行时不支持迁移。
#[tauri::command]
pub fn get_vault_path(state: State<AppState>) -> Result<String, String> {
    let v = lock(&state)?;
    Ok(v.root().to_string_lossy().to_string())
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
    std::fs::write(&path, contents).map_err(|e| e.to_string())
}
