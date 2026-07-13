//! FRB 全量 vault API —— 镜像 Tauri 移动端 `apps/mobile/src-tauri/src/commands.rs`
//! 与 `lib.rs`(AppState/resolve_vault_paths/open_vault_with_fallback)的能力,底下
//! 调的是同一套 `core-model`/`pipeline`/`medme-share`/`dicom`/`parser`,保证保险箱
//! 格式与桌面**逐字节一致**(直接复用 core-model,不另写序列化)。
//!
//! 与 Tauri 版的结构差异只在于「怎么拿到 Vault」:Tauri 用 `tauri::State`(每次
//! 调用由框架注入 `AppState`);FRB 函数是纯自由函数,没有这个注入点,所以这里用
//! 一个进程级 `static VAULT` 替代 `AppState`,`open_vault` 初始化它、其余函数取锁
//! 使用 —— 语义与 Tauri 版的 `AppState`/`VaultPaths` 一致(真相根/派生库路径/
//! 设备 id 一起存,重置/迁移时一并替换)。
use crate::api::dto::*;
use core_model::{DocType, NewDocument, NewOcr, OcrBackendKind, Vault};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// 随应用二进制打包的示例数据(张建国示例病历,corpus/scenarios,文本+PDF,
/// 不含大体积 DICOM——与 Tauri 移动端 `demo-data/` 同一份数据集)。
///
/// Tauri 版靠 `bundle.resources` 把 `demo-data/` 打进应用包、运行时用
/// `app.path().resource_dir()` 定位;FRB 生成的是一个纯 Rust 静态库,没有
/// 「应用资源目录」这个概念,也没有 Tauri 的路径 API。最简单、构建期就固定、
/// 无需 Flutter 端额外打包/解压逻辑的做法是用 `include_dir!` 把这份数据集直接
/// 编译进本 crate 的二进制(~4MB,可接受)。见 `load_demo_data`。
static DEMO_DATA: include_dir::Dir<'_> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/demo-data");

/// 全局 Vault 持有者,镜像 Tauri 的 `AppState`:真相根/派生库路径/设备 id 随
/// Vault 一起存(`reset_vault` 需要同时读写这几样)。`data_dir` 是 App 沙盒 data
/// 目录,存 `device_id` 文件,也是 `ingest_bytes`/`load_demo_data` 的临时文件落点
/// (镜像 Tauri 版用 `app_cache_dir()` 存一次性导入临时文件的做法)。
struct VaultState {
    vault: Vault,
    /// 真相(`objects/` + `log/`)所在目录:本机 `<docs_dir>/vault`,或(开了 iCloud
    /// 同步且容器可用时)iCloud 容器 `<container>/Documents/vault`。
    truth_root: PathBuf,
    db_path: PathBuf,
    device_id: String,
    /// App 沙盒 Documents 目录;本机保险箱固定 `<docs_dir>/vault`(关 iCloud 时复制回这)。
    docs_dir: PathBuf,
    data_dir: PathBuf,
}

static VAULT: OnceLock<Mutex<Option<VaultState>>> = OnceLock::new();

fn vault_cell() -> &'static Mutex<Option<VaultState>> {
    VAULT.get_or_init(|| Mutex::new(None))
}

/// 在已打开的 vault 状态上跑 `f`。恢复被污染的锁而不是让此后每次调用都失败——
/// 镜像 Tauri 版 `commands::lock()` 的理由:Vault 的「真相」是追加式日志 + CAS,
/// 一把被 panic 污染过的锁里的 Vault 仍然可用。
fn with_state<T>(f: impl FnOnce(&VaultState) -> anyhow::Result<T>) -> anyhow::Result<T> {
    let guard = vault_cell().lock().unwrap_or_else(|p| p.into_inner());
    let state = guard
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("保险箱尚未打开,请先调用 open_vault"))?;
    f(state)
}

/// 需要替换 `VaultState` 本身(目前只有 `reset_vault`)时用这个。
fn with_state_mut<T>(f: impl FnOnce(&mut VaultState) -> anyhow::Result<T>) -> anyhow::Result<T> {
    let mut guard = vault_cell().lock().unwrap_or_else(|p| p.into_inner());
    let state = guard
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("保险箱尚未打开,请先调用 open_vault"))?;
    f(state)
}

/// 本机持久设备 id,存在 `<data_dir>/device_id`(沙盒 data 目录,不进保险箱本身——
/// 保险箱可能是个跨设备共享/同步的文件夹,设备 id 必须留在本机)。首次打开时生成
/// 并落盘。镜像 Tauri 版 `lib.rs::machine_device_id`。
fn machine_device_id(data_dir: &Path) -> anyhow::Result<String> {
    let file = data_dir.join("device_id");
    if let Ok(s) = std::fs::read_to_string(&file) {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    let id = core_model::generate_device_id();
    std::fs::write(&file, &id)?;
    Ok(id)
}

/// 影像 study 文档在时间线上显示切片数;非影像文档 slice_count 为 None。
fn doc_summary(v: &Vault, d: &core_model::Document) -> DocumentSummaryDto {
    let mut s = DocumentSummaryDto::from(d);
    if d.doc_type == DocType::ImagingReport {
        if let Ok(n) = v.imaging_instance_count(d.id) {
            if n > 0 {
                s.slice_count = Some(n as i32);
            }
        }
    }
    s
}

/// 打开(或新建)保险箱。iCloud 容器路径由 **Dart 侧经 MethodChannel 解析后传入**
/// (`icloud_container_dir`,容器根目录;不可用/非 iOS 传 `None`)——避免 Rust 框架
/// 反向链接 app target 的 Swift 符号(Flutter 插件框架不允许,会 archive linker 失败)。
///
/// 是否用 iCloud 布局以持久标记 `<data_dir>/icloud_enabled` 为准(enable/disable 写/删)。
/// 开了标记且传入了容器 → 真相在 `<container>/Documents/vault`、派生库在沙盒;否则本机
/// `<docs_dir>/vault`。在解析出的 truth_root 打开失败则回退本机,绝不因 iCloud 问题崩。
pub fn open_vault(
    docs_dir: String,
    data_dir: String,
    icloud_container_dir: Option<String>,
) -> anyhow::Result<()> {
    let docs_dir = PathBuf::from(docs_dir);
    let data_dir = PathBuf::from(data_dir);
    std::fs::create_dir_all(&docs_dir)?;
    std::fs::create_dir_all(&data_dir)?;
    let device_id = machine_device_id(&data_dir)?;

    let local_vault = docs_dir.join("vault");
    let local_db = local_vault.join("medme.db");
    let (truth_root, db_path) =
        resolve_vault_paths(&docs_dir, &data_dir, icloud_container_dir.as_deref());

    let (vault, truth_root, db_path) =
        open_resilient_with_fallback(&truth_root, &db_path, &local_vault, &local_db, &device_id)?;

    let mut guard = vault_cell().lock().unwrap_or_else(|p| p.into_inner());
    *guard = Some(VaultState {
        vault,
        truth_root,
        db_path,
        device_id,
        docs_dir,
        data_dir,
    });
    Ok(())
}

/// 决定真相/派生库路径:开了 iCloud 标记且 Dart 传入了容器根 → 真相在
/// `<container>/Documents/vault`(与旧 Tauri 版路径拼法一致)、派生库在沙盒
/// `<data_dir>/medme.db`;否则本机 `<docs_dir>/vault`(派生库同目录)。
fn resolve_vault_paths(
    docs_dir: &Path,
    data_dir: &Path,
    container: Option<&str>,
) -> (PathBuf, PathBuf) {
    let local_vault = docs_dir.join("vault");
    let local_db = local_vault.join("medme.db");
    if data_dir.join("icloud_enabled").exists() {
        if let Some(c) = container {
            // `container` 是 Dart 拼好的「该成员的 iCloud 目录基」(含 Documents 及
            // 多成员子文件夹),这里只补 `vault`;派生库放**该成员的本机基目录**下
            // (每成员独立、且不进 iCloud——多成员共用 data_dir/medme.db 会撞库)。
            let _ = data_dir;
            let cv = Path::new(c).join("vault");
            return (cv, docs_dir.join("medme.db"));
        }
    }
    (local_vault, local_db)
}

/// 在 `truth_root` 用 `open_split_resilient` 打开(派生库损坏可从日志重建);失败且
/// truth_root 非本机时回退本机沙盒保险箱。返回实际使用的 `(vault, truth_root, db_path)`。
fn open_resilient_with_fallback(
    truth_root: &Path,
    db_path: &Path,
    local_vault: &Path,
    local_db: &Path,
    device_id: &str,
) -> anyhow::Result<(Vault, PathBuf, PathBuf)> {
    match Vault::open_split_resilient(truth_root, db_path, device_id) {
        Ok(v) => Ok((v, truth_root.to_path_buf(), db_path.to_path_buf())),
        Err(_) if truth_root != local_vault => {
            let v = Vault::open_split_resilient(local_vault, local_db, device_id)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            Ok((v, local_vault.to_path_buf(), local_db.to_path_buf()))
        }
        Err(e) => Err(anyhow::anyhow!(e.to_string())),
    }
}

/// 健康档案时间线:就诊组 + 独立文档,按日期倒序(无日期最后)。与桌面/Tauri
/// 移动端的 `load_archive` 同构——复用同一套 core-model 查询。
pub fn load_archive() -> anyhow::Result<Vec<TimelineGroupDto>> {
    with_state(|state| {
        let v = &state.vault;
        v.rebuild_encounters()
            .map_err(|e| anyhow::anyhow!(e.to_string()))?; // 幂等
        let mut groups: Vec<(Option<String>, TimelineGroupDto)> = Vec::new();
        for (enc, docs) in v
            .encounters_with_docs()
            .map_err(|e| anyhow::anyhow!(e.to_string()))?
        {
            let sort = enc.start_date.map(|d| d.to_rfc3339());
            let summary = EncounterSummaryDto::from_encounter(&enc, docs.len() as i64);
            let doc_dtos = docs.iter().map(|d| doc_summary(v, d)).collect();
            groups.push((
                sort,
                TimelineGroupDto::Encounter {
                    encounter: summary,
                    docs: doc_dtos,
                },
            ));
        }
        for d in v
            .standalone_documents()
            .map_err(|e| anyhow::anyhow!(e.to_string()))?
        {
            let sort = d.doc_date.map(|x| x.to_rfc3339());
            groups.push((
                sort,
                TimelineGroupDto::Document {
                    doc: doc_summary(v, &d),
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
    })
}

/// 文档详情:类型/日期 + 来源文件 + 识别文本。与桌面/Tauri 移动端的
/// `get_document` 同构。
pub fn get_document(id: i64) -> anyhow::Result<DocumentDetailDto> {
    with_state(|state| {
        let v = &state.vault;
        let doc = v
            .document_by_id(id)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?
            .ok_or_else(|| anyhow::anyhow!("找不到文档 {id}"))?;
        let sf = v
            .source_file_by_id(doc.source_file_id)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?
            .ok_or_else(|| anyhow::anyhow!("来源文件缺失"))?;
        let ocr_text = v.ocr_text(id).map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let ocr_confidence = v
            .ocr_confidence(id)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let ocr_backend = v
            .ocr_backend(id)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(DocumentDetailDto {
            document: doc_summary(v, &doc),
            source_file: SourceFileMetaDto::from(&sf),
            ocr_text,
            ocr_confidence,
            ocr_backend,
        })
    })
}

/// 一份来源文件的原始字节(图片文档据此渲染缩略图/大图)。与桌面/Tauri 移动端的
/// `read_source_bytes` 同构;移动端目前没有 iCloud 逐出对象的下载触发逻辑
/// (P2 未接 iCloud——见 `open_vault`),直接读盘。
pub fn read_source_bytes(id: i64) -> anyhow::Result<Vec<u8>> {
    with_state(|state| {
        let v = &state.vault;
        let sf = v
            .source_file_by_id(id)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?
            .ok_or_else(|| anyhow::anyhow!("找不到来源文件 {id}"))?;
        let bytes = std::fs::read(v.root_join(&sf.storage_path))?;
        Ok(bytes)
    })
}

/// 渲染一份 DICOM 来源文件的锚点切片为 PNG。
///
/// 安全:`apps/mobile_flutter/rust/Cargo.toml` 给 iOS + 安卓两端都关掉了 `dicom`
/// 的 `codecs` 特性(C/C++ JPEG2000/JPEG-LS 解码器,GHSA-24px 的 RCE 面),与
/// Tauri 移动端 `apps/mobile/src-tauri/Cargo.toml` 的取舍完全一致——桌面才需要
/// 子进程隔离渲染(`dicom_subprocess`),移动端直接用 `medme_share` 提供的进程内
/// 渲染器就是安全的(其文档明确写了这点)。不支持的压缩格式返回错误,前端按现有
/// 「无法预览」的降级处理即可。
pub fn render_dicom_png(id: i64) -> anyhow::Result<Vec<u8>> {
    with_state(|state| {
        let v = &state.vault;
        let sf = v
            .source_file_by_id(id)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?
            .ok_or_else(|| anyhow::anyhow!("找不到来源文件 {id}"))?;
        let bytes = std::fs::read(v.root_join(&sf.storage_path))?;
        medme_share::render_dicom_png_in_process(&bytes)
            .ok_or_else(|| anyhow::anyhow!("无法渲染该 DICOM(暂不支持的压缩格式)"))
    })
}

/// 删除一份文档(用户在 review 队列 / 时间线 / 详情页移除)。追加 `DocumentDeleted`
/// 事件 + 重放,原始字节留在 CAS(见 core-model `delete_document`)。文档不存在 = no-op。
/// 前端删完 `bumpVaultRevision` 刷新即可。
pub fn delete_document(document_id: i64) -> anyhow::Result<()> {
    with_state(|state| {
        state
            .vault
            .delete_document(document_id)
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    })
}

/// 患者档案头(姓名/性别/年龄/记录数)。与桌面/Tauri 移动端同构。
pub fn patient_profile() -> anyhow::Result<PatientProfileDto> {
    with_state(|state| {
        let p = pipeline::patient_profile(&state.vault)?;
        Ok(PatientProfileDto {
            name: p.name,
            gender: p.gender,
            birth_date: p.birth_date,
            age: p.age,
            record_count: p.record_count,
        })
    })
}

/// 跑一次 `pipeline::ingest` 并映射成 `ImportOutcomeDto`。抽取失败(扫描图等)
/// 不致命——原文件已进 CAS,返回 status="failed" 让前端提示「未能识别」而非报错
/// 崩溃。与 Tauri 版 `ingest_one` 同构,但不含 iOS Vision / 安卓 ML Kit 的分岔——
/// Flutter 端已用 `google_mlkit_text_recognition` 识别好图片文本,走
/// `ingest_image_with_text`,这里只处理 PDF/TXT/DICOM 等有文本层/结构化元数据的
/// 文件类型(见 `docs/020_Flutter_Mobile_Rewrite.md` 的 OCR 分工)。
/// 从一份文档的 OCR 文本里识别患者姓名(用于「导错人」核对)。读文本失败或识别不到返回 None。
fn detected_name_for(v: &Vault, doc_id: i64) -> Option<String> {
    v.ocr_text(doc_id)
        .ok()
        .and_then(|t| parser::extract_demographics(&t).name)
}

fn ingest_one(v: &Vault, path: &Path) -> ImportOutcomeDto {
    // Panic firewall:parser/dicom 栈里的 panic 不能一路 unwind 穿过持锁的 Vault、
    // 污染共享 Mutex(与 Tauri 版 `ingest_one` 同一理由)。
    let dispatched = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pipeline::ingest(v, path)
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
            let document_id = v
                .document_by_source_file_id(o.source_file_id)
                .ok()
                .flatten()
                .map(|d| d.id);
            let detected_name = document_id.and_then(|id| detected_name_for(v, id));
            ImportOutcomeDto {
                name: o.name,
                source_file_id: o.source_file_id,
                status,
                doc_type: o.doc_type.map(|d| d.as_str().to_string()),
                document_id,
                detected_name,
            }
        }
        Err(e) => {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            eprintln!("[ingest] failed for {}: {e}", path.display());
            ImportOutcomeDto {
                name,
                source_file_id: 0,
                status: "failed".to_string(),
                doc_type: None,
                document_id: None,
                detected_name: None,
            }
        }
    }
}

/// 采集:对一个真实文件路径(如系统文件选择器返回的路径)跑 ingest,然后重建
/// 就诊分组。PDF/TXT/DICOM 走 `pipeline::ingest`。
pub fn ingest_file(path: String) -> anyhow::Result<ImportOutcomeDto> {
    with_state(|state| {
        let v = &state.vault;
        let outcome = ingest_one(v, Path::new(&path));
        v.rebuild_encounters()
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(outcome)
    })
}

/// 采集(字节直传):Flutter 侧从相机/相册/文件选择器拿到的字节 + 原始文件名。
/// 落到沙盒 data 目录下的一次性临时文件(保留扩展名——`pipeline::mime_for` 靠
/// 扩展名判 MIME/PDF/DICOM)→ 跑 ingest → 重建分组 → 删临时文件。镜像 Tauri 版
/// `ingest_bytes`,只是临时文件目录用 `data_dir`(FRB 没有 `app_cache_dir()`)。
pub fn ingest_bytes(filename: String, data: Vec<u8>) -> anyhow::Result<ImportOutcomeDto> {
    if data.is_empty() {
        anyhow::bail!("空文件,未采集到任何数据");
    }
    if data.len() as u64 > pipeline::MAX_INGEST_BYTES {
        anyhow::bail!(
            "文件过大:{} 字节,超过上限 {} 字节(200MB),已拒绝采集 / file too large",
            data.len(),
            pipeline::MAX_INGEST_BYTES
        );
    }
    let base = Path::new(&filename)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("capture.jpg");
    let safe_name = if Path::new(base).extension().is_some() {
        base.to_string()
    } else {
        format!("{base}.jpg")
    };

    with_state(|state| {
        let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S%f");
        let tmp_dir = state.data_dir.join("medme-ingest").join(stamp.to_string());
        std::fs::create_dir_all(&tmp_dir)?;
        let tmp_path = tmp_dir.join(&safe_name);
        std::fs::write(&tmp_path, &data)?;

        let v = &state.vault;
        let outcome = ingest_one(v, &tmp_path);
        v.rebuild_encounters()
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let _ = std::fs::remove_dir_all(&tmp_dir); // 尽力清理,失败无妨
        Ok(outcome)
    })
}

/// 采集(图片,Flutter 端已用 ML Kit 识别好文本):原始字节先入 CAS(与
/// `pipeline::ingest` 一致,去重同一张图);识别出文字则建 document + ocr_result
/// (backend 固定 `OcrBackendKind::MlKit`,置信度取调用方传入值,与识别引擎无关——
/// 只是把「MedMe 侧落库时统一打上 MlKit 标签」这件事从 Rust 端 OCR 迁到 Flutter
/// 端 OCR,记录里如实标注来源);识别为空则退回文件名元数据(`StoredNoText`),
/// 原件仍可见。落库语义逐字镜像 Tauri 版 `ingest_image_via_vision`/
/// `ingest_image_via_mlkit`,只是识别文本来自参数而非本地再跑一次 OCR。
pub fn ingest_image_with_text(
    name: String,
    bytes: Vec<u8>,
    ocr_text: String,
    confidence: f32,
) -> anyhow::Result<ImportOutcomeDto> {
    if bytes.is_empty() {
        anyhow::bail!("空文件,未采集到任何数据");
    }
    if bytes.len() as u64 > pipeline::MAX_INGEST_BYTES {
        anyhow::bail!(
            "文件过大:{} 字节,超过上限 {} 字节(200MB),已拒绝采集 / file too large",
            bytes.len(),
            pipeline::MAX_INGEST_BYTES
        );
    }
    let base = Path::new(&name)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("capture.jpg");
    let safe_name = if Path::new(base).extension().is_some() {
        base.to_string()
    } else {
        format!("{base}.jpg")
    };

    with_state(|state| {
        let v = &state.vault;
        let mime = pipeline::mime_for(Path::new(&safe_name));
        let imp = v
            .import(&safe_name, mime, &bytes)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let sid = imp.source_file.id;

        let outcome = if imp.deduped
            && v.has_document(sid)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?
        {
            ImportOutcomeDto {
                name: safe_name.clone(),
                source_file_id: sid,
                status: "deduped".to_string(),
                doc_type: None,
                document_id: None,
                detected_name: None,
            }
        } else {
            let text = ocr_text.trim().to_string();
            if !text.is_empty() {
                let doc_type = parser::classify(&text);
                let (doc_date, doc_date_end) = parser::guess_date_range(&text);
                let doc = v
                    .add_document(NewDocument {
                        source_file_id: sid,
                        doc_type: doc_type.clone(),
                        doc_date,
                        doc_date_end,
                        title: Some(safe_name.clone()),
                        language: parser::detect_language(&text),
                        page_count: 1,
                    })
                    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                v.add_ocr(NewOcr {
                    document_id: doc.id,
                    page_no: 1,
                    backend: OcrBackendKind::MlKit,
                    model_version: "mlkit".into(),
                    text: ocr_text,
                    confidence: Some(confidence),
                })
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                let status = if imp.deduped { "backfilled" } else { "new" };
                ImportOutcomeDto {
                    name: safe_name.clone(),
                    source_file_id: sid,
                    status: status.to_string(),
                    doc_type: Some(doc_type.as_str().to_string()),
                    document_id: Some(doc.id),
                    detected_name: parser::extract_demographics(&text).name,
                }
            } else {
                let (doc_date, doc_date_end) = parser::guess_date_range(&safe_name);
                let doc_type = parser::classify(&safe_name);
                let doc = v
                    .add_document(NewDocument {
                        source_file_id: sid,
                        doc_type: doc_type.clone(),
                        doc_date,
                        doc_date_end,
                        title: Some(safe_name.clone()),
                        language: None,
                        page_count: 1,
                    })
                    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                ImportOutcomeDto {
                    name: safe_name.clone(),
                    source_file_id: sid,
                    status: "stored_no_text".to_string(),
                    doc_type: Some(doc_type.as_str().to_string()),
                    document_id: Some(doc.id),
                    detected_name: None, // 无文本,识别不到名字
                }
            }
        };
        v.rebuild_encounters()
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(outcome)
    })
}

/// 端到端加密分享:复用 `medme_share::share::build_encrypted_share`,把全部病历
/// 打包成自包含加密 HTML 写进保险箱 `shares/` 目录,返回口令、记录数、字节数与
/// 文件路径。与桌面/Tauri 移动端同构;安全性说明见 `render_dicom_png` 的 doc
/// (进程内 DICOM 渲染在移动端是安全的,`codecs` 特性已关)。
pub fn create_share(expires_days: i64) -> anyhow::Result<ShareResultDto> {
    let days: u32 = expires_days
        .try_into()
        .map_err(|_| anyhow::anyhow!("expires_days 取值无效:{expires_days}"))?;

    with_state(|state| {
        let v = &state.vault;
        let (html, passphrase, record_count) = medme_share::share::build_encrypted_share(
            v,
            days,
            &medme_share::render_dicom_png_in_process,
        )
        .map_err(|e| anyhow::anyhow!(e))?;
        let byte_size = html.len() as i64;
        let sha256 = core_model::cas::sha256_hex(html.as_bytes());

        let shares_dir = state.truth_root.join("shares");
        std::fs::create_dir_all(&shares_dir)?;
        let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let dest = shares_dir.join(format!("medme-share-{stamp}.html"));
        std::fs::write(&dest, html)?;

        let expires = (chrono::Utc::now() + chrono::Duration::days(days as i64)).to_rfc3339();
        v.record_share(&sha256, record_count, &expires)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(ShareResultDto {
            passphrase,
            record_count,
            byte_size,
            path: dest.to_string_lossy().to_string(),
        })
    })
}

/// 导出时间线:复用 `medme_share::export::build_timeline_html_ranged`,把时间线
/// 渲染成未加密、可打印的自包含 HTML 写进保险箱 `shares/` 目录(与加密分享共用
/// 同一目录——都是本机生成、交给系统分享 sheet 的临时导出件)。
///
/// `from_date` / `to_date` 为可选的 `YYYY-MM-DD`(前端日期选择器传入);任一为空
/// 表示该侧不限,两者都为空即全量导出。`from` 取当天 00:00、`to` 取当天 23:59:59
/// (含端点)。无 `doc_date` 的记录仅在完全不筛选时纳入(见共享 crate 的说明)。
pub fn export_timeline_html(
    from_date: Option<String>,
    to_date: Option<String>,
) -> anyhow::Result<ExportResultDto> {
    let parse = |s: &str, end_of_day: bool| -> anyhow::Result<chrono::DateTime<chrono::Utc>> {
        let d = chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
            .map_err(|e| anyhow::anyhow!("日期格式应为 YYYY-MM-DD:{e}"))?;
        let t = if end_of_day {
            d.and_hms_opt(23, 59, 59)
        } else {
            d.and_hms_opt(0, 0, 0)
        }
        .ok_or_else(|| anyhow::anyhow!("无效日期"))?;
        Ok(t.and_utc())
    };
    let from = from_date
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(|s| parse(s, false))
        .transpose()?;
    let to = to_date
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(|s| parse(s, true))
        .transpose()?;
    with_state(|state| {
        let v = &state.vault;
        let (html, record_count) = medme_share::export::build_timeline_html_ranged(
            v,
            &medme_share::render_dicom_png_in_process,
            from,
            to,
        )
        .map_err(|e| anyhow::anyhow!(e))?;
        let byte_size = html.len() as i64;
        let sha256 = core_model::cas::sha256_hex(html.as_bytes());

        let shares_dir = state.truth_root.join("shares");
        std::fs::create_dir_all(&shares_dir)?;
        let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let dest = shares_dir.join(format!("medme-timeline-{stamp}.html"));
        std::fs::write(&dest, html)?;

        v.record_export("timeline_html", &sha256, record_count)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(ExportResultDto {
            record_count,
            byte_size,
            path: dest.to_string_lossy().to_string(),
        })
    })
}

/// 递归收集 `include_dir!` 打进二进制的示例数据集里的全部文件。
fn collect_demo_files<'a>(dir: &'a include_dir::Dir<'a>, out: &mut Vec<&'a include_dir::File<'a>>) {
    out.extend(dir.files());
    for sub in dir.dirs() {
        collect_demo_files(sub, out);
    }
}

/// 一键「载入示例数据」:把编译进本 crate 的张建国示例病历(见 `DEMO_DATA`)
/// 批量导入保险箱,让测试者无需手动选文件就能看到 健康档案。按路径排序保证
/// 可复现;`pipeline::ingest` 去重,重复点击安全。返回成功处理的文件数。
///
/// 与 Tauri 版的差异只在「示例数据从哪来」:Tauri 版随 `bundle.resources` 打包、
/// 运行时用 `resource_dir()` 定位;这里没有「应用资源目录」,数据集直接编译进
/// 二进制(`DEMO_DATA`),运行时落一份到 `data_dir` 下的临时目录再喂给
/// `pipeline::ingest`(它按路径操作,不接受内存字节),用完即删。
pub fn load_demo_data() -> anyhow::Result<i64> {
    with_state(|state| {
        let v = &state.vault;
        let mut files: Vec<&include_dir::File<'_>> = Vec::new();
        collect_demo_files(&DEMO_DATA, &mut files);
        files.sort_by_key(|f| f.path().to_path_buf());

        let tmp_root = state.data_dir.join("medme-demo-data");
        std::fs::create_dir_all(&tmp_root)?;
        let mut count = 0i64;
        for f in &files {
            let tmp_path = tmp_root.join(f.path());
            if let Some(parent) = tmp_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&tmp_path, f.contents())?;
            // Panic firewall(与 `ingest_one` 同一理由):parser/dicom 栈里的
            // panic 不能一路 unwind 穿过持锁的 Vault。
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                pipeline::ingest(v, &tmp_path)
            }));
            match result {
                Ok(Ok(_)) => count += 1,
                Ok(Err(e)) => {
                    eprintln!("[demo-data] ingest failed for {}: {e}", tmp_path.display())
                }
                Err(_) => eprintln!(
                    "[demo-data] ingest panicked (isolated) for {}",
                    tmp_path.display()
                ),
            }
        }
        let _ = std::fs::remove_dir_all(&tmp_root); // 尽力清理,失败无妨
        v.rebuild_encounters()
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(count)
    })
}

/// 「清空保险箱 · 重置」:删掉当前真相目录(`truth_root`)+ 派生库
/// (`db_path`),再用 `open_split_resilient` 在同一位置重建。之后 `load_archive`
/// 会返回空。与桌面/Tauri 移动端的 `reset_vault` 同构,包括同一条安全兜底:
/// `truth_root` 必须是一个名为 `vault` 的目录,防止误删沙盒其它内容。
pub fn reset_vault() -> anyhow::Result<()> {
    with_state_mut(|state| {
        if state.truth_root.file_name().and_then(|n| n.to_str()) != Some("vault") {
            anyhow::bail!("保险箱路径异常,已中止重置");
        }
        if state.truth_root.exists() {
            std::fs::remove_dir_all(&state.truth_root)?;
        }
        if state.db_path.exists() && !state.db_path.starts_with(&state.truth_root) {
            std::fs::remove_file(&state.db_path)?;
        }
        let fresh =
            Vault::open_split_resilient(&state.truth_root, &state.db_path, &state.device_id)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        state.vault = fresh; // 旧 Vault(连接/日志句柄)在此被 drop
        Ok(())
    })
}

/// iCloud 同步状态占位。P2 阶段恒为「不可用/未开启」——真正的容器解析/开关逻辑
/// 是 iOS-only 且依赖 Tauri 路径 API(见 Tauri 版 `apps/mobile/src-tauri/src/icloud.rs`),
/// 不能直接照搬进这个平台无关的 FFI 层,留给 P5 用 Flutter/iOS 原生桥重做
/// (见 `docs/020_Flutter_Mobile_Rewrite.md` 的「同步(iCloud,iOS)」一节)。
/// iCloud 同步是否已在本设备开启(读持久标记)。`available`(容器是否可解析)由
/// Dart 侧经 MethodChannel 判断——Rust 拿不到容器,恒返回 false,Dart 覆盖。
pub fn icloud_status() -> IcloudStatusDto {
    let enabled = with_state(|s| Ok(s.data_dir.join("icloud_enabled").exists())).unwrap_or(false);
    IcloudStatusDto {
        available: false,
        enabled,
    }
}

/// 开启 iCloud 同步:把保险箱真相迁进 iCloud 容器 `<container_dir>/Documents/vault`,
/// 派生库留沙盒,写持久标记。容器路径由 Dart 经 MethodChannel 解析后传入。迁移用
/// core-model `relocate_to`(搬 objects/log/db/VERSION;容器里已有别设备的 vault 则
/// adopt+merge)——与已验证的 Tauri #38 同一套安全操作。幂等。
pub fn enable_icloud_sync(container_dir: String) -> anyhow::Result<()> {
    // `container_dir` 是 Dart 拼好的「该成员 iCloud 目录基」(含 Documents 及多成员
    // 子文件夹),这里只补 `vault`。
    let container_vault = Path::new(&container_dir).join("vault");
    with_state_mut(|state| {
        if state.truth_root == container_vault {
            std::fs::write(state.data_dir.join("icloud_enabled"), "1")?;
            return Ok(());
        }
        // 派生库放该成员本机基目录(每成员独立,不进 iCloud),避免多成员撞库。
        let sandbox_db = state.docs_dir.join("medme.db");
        state
            .vault
            .relocate_to(&container_vault)
            .map_err(|e| anyhow::anyhow!(format!("迁移保险箱到 iCloud 失败:{e}")))?;
        let _ = std::fs::remove_file(container_vault.join("medme.db"));
        let fresh = Vault::open_split(&container_vault, &sandbox_db, &state.device_id)
            .map_err(|e| anyhow::anyhow!(format!("在 iCloud 容器打开保险箱失败:{e}")))?;
        state.vault = fresh;
        state.truth_root = container_vault;
        state.db_path = sandbox_db;
        std::fs::write(state.data_dir.join("icloud_enabled"), "1")?;
        Ok(())
    })
}

/// 关闭 iCloud 同步:把真相从容器**复制**回本机 `<docs_dir>/vault`(容器副本保留),
/// 本地重开派生库,清标记 + 沙盒 iCloud 派生库。用 `copy_to` 只复制不删源。幂等。
pub fn disable_icloud_sync() -> anyhow::Result<()> {
    with_state_mut(|state| {
        let local_vault = state.docs_dir.join("vault");
        let local_db = local_vault.join("medme.db");
        if state.truth_root == local_vault {
            let _ = std::fs::remove_file(state.data_dir.join("icloud_enabled"));
            return Ok(());
        }
        state
            .vault
            .copy_to(&local_vault)
            .map_err(|e| anyhow::anyhow!(format!("把保险箱复制回本机失败:{e}")))?;
        let fresh = Vault::open_split(&local_vault, &local_db, &state.device_id)
            .map_err(|e| anyhow::anyhow!(format!("在本机打开保险箱失败:{e}")))?;
        state.vault = fresh;
        state.truth_root = local_vault;
        state.db_path = local_db;
        let _ = std::fs::remove_file(state.data_dir.join("icloud_enabled"));
        let _ = std::fs::remove_file(state.data_dir.join("medme.db"));
        Ok(())
    })
}
