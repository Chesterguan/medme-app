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

// SECURITY/robustness: recover a poisoned lock instead of failing every command.
// The ingest entry points now run `pipeline::ingest` under `catch_unwind` (see
// `ingest_one`), so a bad file can't poison this mutex in the first place. If some
// other path ever panics while holding the guard, `into_inner()` keeps the Vault
// usable rather than bricking the app past one bad operation — the Vault's truth is
// the append-only log + CAS, so a recovered guard is safe.
fn lock<'a>(s: &'a State<'a, AppState>) -> Result<std::sync::MutexGuard<'a, Vault>, String> {
    Ok(s.vault
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()))
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
        groups.push((
            sort,
            TimelineGroup::Document {
                doc: doc_summary(&v, &d),
            },
        ));
    }
    groups.sort_by(|a, b| match (&a.0, &b.0) {
        (Some(x), Some(y)) => y.cmp(x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    Ok(groups.into_iter().map(|(_, g)| g).collect())
}

/// 跑一次 ingest 并映射为前端 `ImportOutcome`。抽取失败(扫描图等)不致命
/// —— 原文件已进 CAS,返回 status="failed" 让前端提示「未能识别」而非报错崩溃。
///
/// iOS 与桌面的分岔点在 `ingest_dispatch`:iOS 上的**图片**走 Apple Vision(见
/// `vision` 模块 / `ingest_image_via_vision`),其余一切(以及桌面/host 构建)沿用
/// 未改动的 `pipeline::ingest`。
fn ingest_one(app: &tauri::AppHandle, v: &Vault, path: &std::path::Path) -> ImportOutcome {
    // Panic firewall: a panic inside the parser/dicom/ocr/vision stack (defense-in-depth
    // over their internal guards) is caught here and turned into a `failed` outcome,
    // so it can't unwind the command thread past the held Vault guard and poison the
    // shared mutex. `AssertUnwindSafe` is needed because `&Vault` isn't `UnwindSafe`;
    // on a caught panic we touch no half-mutated state, just report the failure.
    let dispatched = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ingest_dispatch(app, v, path)
    })) {
        Ok(r) => r,
        Err(_) => Err(anyhow::anyhow!("导入时发生内部错误(已隔离),该文件已跳过")),
    };
    match dispatched {
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
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            eprintln!("[ingest] failed for {}: {e}", path.display());
            ImportOutcome {
                name,
                source_file_id: 0,
                status: "failed".to_string(),
                doc_type: None,
            }
        }
    }
}

/// ingest 分岔:iOS 图片 → Apple Vision;其余 → 共享 `pipeline::ingest`(桌面行为不变)。
fn ingest_dispatch(
    app: &tauri::AppHandle,
    v: &Vault,
    path: &std::path::Path,
) -> anyhow::Result<pipeline::IngestOutcome> {
    let _ = app; // 非 iOS 上未用
    #[cfg(target_os = "ios")]
    {
        if is_image(path) {
            return ingest_image_via_vision(v, path);
        }
    }
    pipeline::ingest(v, path)
}

/// 图片 MIME 判定(仅 iOS 用于路由到 Vision)。
#[cfg(target_os = "ios")]
fn is_image(path: &std::path::Path) -> bool {
    matches!(
        pipeline::mime_for(path),
        "image/jpeg" | "image/png" | "image/tiff"
    )
}

/// iOS 图片 OCR:Apple Vision(设备端、离线、无模型下载)替代沙盒里跑不通的 oar-ocr。
///
/// 复刻 `pipeline::ingest` 图片分支的语义,但文本取自 Vision:先把原始字节存进 CAS
/// (「真相」),识别出文字则按文本建 document + ocr_result(后端 `AppleVision`,带平均
/// 置信度)并据文本分类/取日期;识别为空则退回文件名元数据(StoredNoText),原件仍可见。
/// 桌面/`pipeline`/`ocr` 一律不受影响。
#[cfg(target_os = "ios")]
fn ingest_image_via_vision(
    v: &Vault,
    path: &std::path::Path,
) -> anyhow::Result<pipeline::IngestOutcome> {
    use core_model::{NewDocument, NewOcr, OcrBackendKind};
    use pipeline::{IngestOutcome, IngestStatus};

    // 体积闸门(与 pipeline::ingest 同一上限):iOS 图片走 Vision 分支时也不 slurp
    // 一份超大文件进内存,先看 metadata 大小再读。
    let len = std::fs::metadata(path)?.len();
    if len > pipeline::MAX_INGEST_BYTES {
        anyhow::bail!(
            "文件过大:{len} 字节,超过上限 {} 字节(200MB),已拒绝导入 / file too large",
            pipeline::MAX_INGEST_BYTES
        );
    }
    let bytes = std::fs::read(path)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    // 原始字节先入 CAS(与 pipeline 一致,去重同一张图)。
    let imp = v.import(&name, pipeline::mime_for(path), &bytes)?;
    let sid = imp.source_file.id;
    if imp.deduped && v.has_document(sid)? {
        return Ok(IngestOutcome {
            source_file_id: sid,
            name,
            status: IngestStatus::Deduped,
            doc_type: None,
        });
    }

    // 设备端 Apple Vision OCR(离线)。
    let vision = crate::vision::recognize(path)?;
    let text = vision.text.trim().to_string();

    if !text.is_empty() {
        let doc_type = parser::classify(&text);
        let (doc_date, doc_date_end) = parser::guess_date_range(&text);
        let doc = v.add_document(NewDocument {
            source_file_id: sid,
            doc_type: doc_type.clone(),
            doc_date,
            doc_date_end,
            title: Some(name.clone()),
            language: parser::detect_language(&text),
            page_count: 1,
        })?;
        v.add_ocr(NewOcr {
            document_id: doc.id,
            page_no: 1,
            backend: OcrBackendKind::AppleVision,
            model_version: "apple-vision".into(),
            text: vision.text,
            confidence: Some(vision.confidence),
        })?;
        let status = if imp.deduped {
            IngestStatus::Backfilled
        } else {
            IngestStatus::New
        };
        Ok(IngestOutcome {
            source_file_id: sid,
            name,
            status,
            doc_type: Some(doc_type),
        })
    } else {
        // Vision 未识别出文字:退回文件名元数据(与 pipeline 的 StoredNoText 同构),
        // 不建 ocr_result;原件已永存,时间线仍可见、可出示。
        let (doc_date, doc_date_end) = parser::guess_date_range(&name);
        let doc_type = parser::classify(&name);
        v.add_document(NewDocument {
            source_file_id: sid,
            doc_type: doc_type.clone(),
            doc_date,
            doc_date_end,
            title: Some(name.clone()),
            language: None,
            page_count: 1,
        })?;
        Ok(IngestOutcome {
            source_file_id: sid,
            name,
            status: IngestStatus::StoredNoText,
            doc_type: Some(doc_type),
        })
    }
}

/// 采集(mobile P1):对一张拍摄/选取的文件跑 ingest,然后重建就诊分组。
/// 单文件版本的 `import_paths`。桌面/带真实沙盒路径的场景仍可用(例如插件返回路径)。
#[tauri::command]
pub fn ingest_file(
    app: tauri::AppHandle,
    state: State<AppState>,
    path: String,
) -> Result<ImportOutcome, String> {
    let v = lock(&state)?;
    let outcome = ingest_one(&app, &v, std::path::Path::new(&path));
    v.rebuild_encounters().map_err(|e| e.to_string())?;
    Ok(outcome)
}

/// 采集(相机/相册直传):前端从 `<input type=file capture=environment>` 拿到 `File`
/// 后,把字节 + 原始文件名直接传进来。这样绕开了 Web File API 拿不到的沙盒路径 ——
/// iOS WKWebView 里选中的照片只有 `File` 对象,没有可读的文件系统路径。
///
/// 流程:字节落到 App 缓存目录下的一次性临时文件(**保留原扩展名** —— pipeline 靠
/// 扩展名判 MIME / PDF / DICOM,见 `pipeline::mime_for`)→ 跑同一套 ingest 管线入库
/// → 重建就诊分组 → 删掉临时文件(「真相」已进 CAS/日志,临时件用完即弃)。
#[tauri::command]
pub fn ingest_bytes(
    app: tauri::AppHandle,
    state: State<AppState>,
    filename: String,
    data: Vec<u8>,
) -> Result<ImportOutcome, String> {
    use tauri::Manager;
    if data.is_empty() {
        return Err("空文件,未采集到任何数据".to_string());
    }
    // 体积闸门:在把 payload 落盘 / 读入管线之前就拒绝超大字节流,避免一份几个 GB
    // 的畸形/敌意上传把进程 OOM(与 pipeline::ingest 的文件大小上限同一常量)。
    if data.len() as u64 > pipeline::MAX_INGEST_BYTES {
        return Err(format!(
            "文件过大:{} 字节,超过上限 {} 字节(200MB),已拒绝采集 / file too large",
            data.len(),
            pipeline::MAX_INGEST_BYTES
        ));
    }
    // 净化文件名:只取基名防路径穿越;缺扩展名兜底 .jpg(相机默认输出 JPEG)。
    let base = std::path::Path::new(&filename)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("capture.jpg");
    let safe_name = if std::path::Path::new(base).extension().is_some() {
        base.to_string()
    } else {
        format!("{base}.jpg")
    };

    // 每次采集用一个唯一子目录,里面放「真实文件名」——这样入库后的 original_name
    // 就是用户可读的名字,而不是带时间戳的临时名。用后整目录删除。
    let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S%f");
    let tmp_dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("medme-ingest")
        .join(stamp.to_string());
    std::fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;
    let tmp_path = tmp_dir.join(&safe_name);
    std::fs::write(&tmp_path, &data).map_err(|e| format!("写入临时文件失败:{e}"))?;

    let outcome = {
        let v = lock(&state)?;
        let o = ingest_one(&app, &v, &tmp_path);
        v.rebuild_encounters().map_err(|e| e.to_string())?;
        o
    };
    let _ = std::fs::remove_dir_all(&tmp_dir); // 尽力清理,失败无妨
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

/// 读取一份已生成的加密分享 `.html` 文件字节。前端据此构造 `File`,调起 iOS 系统
/// 「分享」sheet(navigator.share)把文件发给医生。安全:只允许读取保险箱 `shares/`
/// 目录下的文件,`canonicalize` 后校验前缀,杜绝任意路径读取 / 路径穿越。
#[tauri::command]
pub fn read_share_bytes(
    state: State<AppState>,
    path: String,
) -> Result<tauri::ipc::Response, String> {
    let shares_dir = state
        .vault_dir
        .join("shares")
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let target = std::path::PathBuf::from(&path)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !target.starts_with(&shares_dir) {
        return Err("非法的分享文件路径".to_string());
    }
    let bytes = std::fs::read(&target).map_err(|e| e.to_string())?;
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
        // Panic firewall (same rationale as ingest_one): a panic in the parser/dicom/ocr
        // stack must not unwind past the held Vault guard and poison the mutex.
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pipeline::ingest(&v, path)));
        match result {
            Ok(Ok(_)) => count += 1,
            Ok(Err(e)) => eprintln!("[demo-data] ingest failed for {}: {e}", path.display()),
            Err(_) => eprintln!(
                "[demo-data] ingest panicked (isolated) for {}",
                path.display()
            ),
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
    let mut guard = state
        .vault
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(|e| format!("清空保险箱失败:{e}"))?;
    }
    let fresh = Vault::open(dir).map_err(|e| format!("重建保险箱失败:{e}"))?;
    *guard = fresh; // 旧 Vault(连接/日志句柄)在此被 drop
    Ok(())
}
