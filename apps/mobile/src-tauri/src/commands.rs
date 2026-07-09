//! Tauri commands for the mobile app — thin wrappers over the shared crates,
//! mirroring the desktop app's logic (which now lives in the crates).
//! Milestone 1: 采集(ingest)+ 健康档案(archive view)+ 加密分享(share).
use crate::dto::*;
use core_model::{DocType, Document, Vault};
use std::sync::Mutex;
use tauri::State;

pub struct AppState {
    pub vault: Mutex<Vault>,
    /// 保险箱根目录(iOS 沙盒 Documents 下)。展示用 + 分享文件落盘位置。
    pub vault_dir: std::path::PathBuf,
}

fn lock<'a>(s: &'a State<'a, AppState>) -> Result<std::sync::MutexGuard<'a, Vault>, String> {
    s.vault.lock().map_err(|_| "vault lock poisoned".to_string())
}

/// 影像 study 文档在时间线上显示切片数;非影像文档 slice_count 为 None。
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

/// 健康档案时间线:就诊组 + 独立文档,按日期倒序(无日期最后)。
/// 与桌面 `list_timeline_grouped` 同构 —— 复用同一套 core-model 查询。
#[tauri::command]
pub fn load_archive(state: State<AppState>) -> Result<Vec<TimelineGroup>, String> {
    let v = lock(&state)?;
    v.rebuild_encounters().map_err(|e| e.to_string())?; // 幂等
    let mut groups: Vec<(Option<String>, TimelineGroup)> = Vec::new();
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
        groups.push((sort, TimelineGroup::Document { doc: doc_summary(&v, &d) }));
    }
    groups.sort_by(|a, b| match (&a.0, &b.0) {
        (Some(x), Some(y)) => y.cmp(x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    Ok(groups.into_iter().map(|(_, g)| g).collect())
}

/// 采集(mobile P1):对一张拍摄/选取的文件跑 pipeline ingest,然后重建就诊分组。
/// 单文件版本的 `import_paths`。
#[tauri::command]
pub fn ingest_file(state: State<AppState>, path: String) -> Result<ImportOutcome, String> {
    let v = lock(&state)?;
    let p = std::path::Path::new(&path);
    let outcome = match pipeline::ingest(&v, p) {
        Ok(o) => {
            let status = match o.status {
                pipeline::IngestStatus::New => "new",
                pipeline::IngestStatus::Backfilled => "backfilled",
                pipeline::IngestStatus::Deduped => "deduped",
                pipeline::IngestStatus::StoredNoText => "stored_no_text",
                pipeline::IngestStatus::InstanceAttached => "instance_attached",
            }
            .to_string();
            ImportOutcome {
                name: o.name,
                source_file_id: o.source_file_id,
                status,
                doc_type: o.doc_type.map(|d| d.as_str().to_string()),
            }
        }
        Err(e) => {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.clone());
            eprintln!("[ingest] failed for {}: {e}", p.display());
            ImportOutcome {
                name,
                source_file_id: 0,
                status: "failed".to_string(),
                doc_type: None,
            }
        }
    };
    v.rebuild_encounters().map_err(|e| e.to_string())?;
    Ok(outcome)
}

/// 文档详情(mobile P2):点开一份记录时拉取类型/日期/来源 + 识别文本。
/// 复用 core-model 查询,与桌面 `get_document` 同构;移动端不含 DICOM 阅片,
/// 影像文档只给元信息与文本,原图/缩略由前端按需 `read_source_bytes`。
#[tauri::command]
pub fn get_document(state: State<AppState>, id: i64) -> Result<DocumentDetail, String> {
    let v = lock(&state)?;
    let doc = v
        .document_by_id(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("找不到文档 {id}"))?;
    let sf = v
        .source_file_by_id(doc.source_file_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "来源文件缺失".to_string())?;
    let ocr_text = v.ocr_text(id).map_err(|e| e.to_string())?;
    let ocr_confidence = v.ocr_confidence(id).map_err(|e| e.to_string())?;
    let ocr_backend = v.ocr_backend(id).map_err(|e| e.to_string())?;
    Ok(DocumentDetail {
        document: doc_summary(&v, &doc),
        source_file: SourceFileMeta::from(&sf),
        ocr_text,
        ocr_confidence,
        ocr_backend,
    })
}

/// 一份来源文件的原始字节(图片文档在详情页据此渲染缩略图)。与桌面同构。
#[tauri::command]
pub fn read_source_bytes(state: State<AppState>, id: i64) -> Result<tauri::ipc::Response, String> {
    let v = lock(&state)?;
    let sf = v
        .source_file_by_id(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("找不到来源文件 {id}"))?;
    let path = v.root_join(&sf.storage_path);
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// 患者档案头(姓名/性别/年龄/记录数)—— 健康档案页顶部展示。
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

/// 端到端加密分享:复用 `medme_share::share::build_encrypted_share`,把全部病历
/// 打包成自包含加密 HTML 写进沙盒 `shares/` 目录(M2 再接系统「分享」sheet 导出),
/// 返回口令、记录数、字节数与文件路径。默认有效期 5 天。
#[tauri::command]
pub fn create_share(
    state: State<AppState>,
    expires_days: Option<u32>,
) -> Result<ShareResult, String> {
    let v = lock(&state)?;
    let days = expires_days.unwrap_or(5);
    let (html, passphrase, record_count) = medme_share::share::build_encrypted_share(&v, days)?;
    let byte_size = html.len() as i64;
    let sha256 = core_model::cas::sha256_hex(html.as_bytes());

    let shares_dir = state.vault_dir.join("shares");
    std::fs::create_dir_all(&shares_dir).map_err(|e| e.to_string())?;
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let dest = shares_dir.join(format!("medme-share-{stamp}.html"));
    std::fs::write(&dest, html).map_err(|e| e.to_string())?;

    let expires = (chrono::Utc::now() + chrono::Duration::days(days as i64)).to_rfc3339();
    v.record_share(&sha256, record_count, &expires)
        .map_err(|e| e.to_string())?;
    Ok(ShareResult {
        passphrase,
        record_count,
        byte_size,
        path: dest.to_string_lossy().to_string(),
    })
}

/// 一键「载入示例数据」:把随应用打包的张建国示例病历(demo-data/,文本+PDF,
/// 不含大体积 DICOM —— 详细阅片交给桌面/在线查看器)批量导入保险箱,让测试者
/// 无需手动选文件就能看到 健康档案。按路径排序保证可复现;pipeline::ingest 去重,
/// 重复点击安全。返回处理的文件数。
#[tauri::command]
pub fn load_demo_data(app: tauri::AppHandle, state: State<AppState>) -> Result<usize, String> {
    use tauri::Manager;
    let dir = app
        .path()
        .resource_dir()
        .map_err(|e| e.to_string())?
        .join("demo-data");
    if !dir.is_dir() {
        return Err("示例数据未随应用打包,无法加载".to_string());
    }
    let mut files = Vec::new();
    collect_files_recursive(&dir, &mut files);
    files.sort();

    let v = lock(&state)?;
    let mut count = 0usize;
    for path in &files {
        match pipeline::ingest(&v, path) {
            Ok(_) => count += 1,
            Err(e) => eprintln!("[demo-data] ingest failed for {}: {e}", path.display()),
        }
    }
    v.rebuild_encounters().map_err(|e| e.to_string())?;
    Ok(count)
}

/// 递归收集目录下全部常规文件。
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

/// 保险箱根目录路径(设置/我的页展示)。
/// TODO iCloud container:v1.1 迁移到 iCloud container,实现与桌面自动同步。
#[tauri::command]
pub fn get_vault_path(state: State<AppState>) -> String {
    state.vault_dir.to_string_lossy().to_string()
}

/// 「清空保险箱 · 重置」:安全地抹掉本 App 的保险箱并重建一个空的,
/// 让示例数据/已导入内容可逆(载入 → 试用 → 清空 → 从头开始)。
///
/// 保险箱的「真相」= `objects/`(内容寻址存储)+ `log/`(追加式事件日志),
/// `medme.db` 只是派生缓存(见 core-model)。因此重置 = 删掉整个 vault_dir
/// 再用 `Vault::open` 重建 —— 与 lib.rs 里首次创建保险箱的方式完全一致,
/// 之后 `load_archive` 会返回空。
///
/// 安全性:只删 `state.vault_dir` 这一个目录,绝不触碰它之外的任何东西。
/// 幂等:目录不存在也不报错(重复点击安全)。Unix 上即便旧连接仍打开着
/// 被删的 `medme.db`,inode 依然有效,替换 `*guard` 时旧 Vault 才被 drop。
#[tauri::command]
pub fn reset_vault(state: State<AppState>) -> Result<(), String> {
    let dir = &state.vault_dir;
    // 兜底:必须是一个名为 `vault` 的目录,防止误删沙盒其它内容。
    if dir.file_name().and_then(|n| n.to_str()) != Some("vault") {
        return Err("保险箱路径异常,已中止重置".to_string());
    }
    let mut guard = state.vault.lock().map_err(|_| "vault lock poisoned".to_string())?;
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(|e| format!("清空保险箱失败:{e}"))?;
    }
    let fresh = Vault::open(dir).map_err(|e| format!("重建保险箱失败:{e}"))?;
    *guard = fresh; // 旧 Vault(连接/日志句柄)在此被 drop
    Ok(())
}
