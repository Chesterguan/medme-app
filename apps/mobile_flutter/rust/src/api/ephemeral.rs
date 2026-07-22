//! 临时会话(即焚)—— 「医生代拍病人纸质材料」流程专用的**平行** vault cell
//! (设计见 `docs/plan-mobile-viewer-极致.md` 的「医生代拍」章节)。
//!
//! 与 `api::vault::VAULT`(医生自己的保险箱)完全独立:不同的进程级 `static`、
//! 不同的磁盘根(iOS `getTemporaryDirectory()` 下的一次性子目录,绝不进
//! docs/vault/profiles 子树)、一次性随机 `device_id`(`core_model::generate_device_id`,
//! 不落盘、不用 `machine_device_id`——分享件不该带医生自己的设备身份)。这样任何
//! 走神的调用**结构上不可能**读到/写到医生自己的病历,也不可能把病人数据误认成
//! 医生自己的档案。
//!
//! 核心医疗逻辑(ingest/load_archive/create_share)与 `api::vault` **共用同一套
//! `VaultState` + `*_core` 自由函数**,这里只负责「怎么拿到这个 cell 的
//! `VaultState`」与「用完即焚」,不重复维护任何 OCR/落库/加密判断。
use crate::api::dto::*;
use crate::api::vault::{
    create_share_core, ingest_bytes_core, ingest_image_with_text_core, load_archive_core,
    VaultState,
};
use core_model::Vault;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// 会话目录名前缀,`ephemeral_sweep` 据此识别、清理崩溃/异常退出留下的残留目录。
const EPHEMERAL_DIR_PREFIX: &str = "ephemeral-";

static EPHEMERAL: OnceLock<Mutex<Option<VaultState>>> = OnceLock::new();

fn ephemeral_cell() -> &'static Mutex<Option<VaultState>> {
    EPHEMERAL.get_or_init(|| Mutex::new(None))
}

/// 在已开始的临时会话上跑 `f`。与 `vault::with_state` 同一恢复被污染锁的理由。
fn with_ephemeral<T>(f: impl FnOnce(&VaultState) -> anyhow::Result<T>) -> anyhow::Result<T> {
    let guard = ephemeral_cell().lock().unwrap_or_else(|p| p.into_inner());
    let state = guard
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("临时会话尚未开始,请先调用 ephemeral_begin"))?;
    f(state)
}

/// 开始一次临时会话:在 `<cache_dir>/ephemeral-<随机>/` 下建全新空箱并打开。
///
/// `cache_dir` 由 Dart 侧传入 iOS `getTemporaryDirectory()`(不进 iCloud 备份,系统
/// 可随时清空——即焚语义与「系统可能替我们清」互为兜底)。会话目录后缀取一次性随机
/// `device_id` 的前 16 位:既避免与其它并发/历史会话撞名,又不必再引入一个独立的
/// 随机源。`device_id` 本身**不落盘、不复用** `machine_device_id`——分享件因此不带
/// 医生本机的设备身份。
pub fn ephemeral_begin(cache_dir: String) -> anyhow::Result<()> {
    let cache_root = PathBuf::from(cache_dir);
    std::fs::create_dir_all(&cache_root)?;

    let device_id = core_model::generate_device_id();
    let session_root = cache_root.join(format!("{EPHEMERAL_DIR_PREFIX}{}", &device_id[..16]));
    if session_root.exists() {
        // 极小概率的目录名碰撞(或上次残留未被 sweep 清掉):清空重来,绝不复用旧内容。
        std::fs::remove_dir_all(&session_root)?;
    }
    std::fs::create_dir_all(&session_root)?;

    let truth_root = session_root.join("vault");
    let db_path = truth_root.join("medme.db");
    let data_dir = session_root.join("data"); // ingest 临时文件等落这里
    std::fs::create_dir_all(&data_dir)?;

    let vault = Vault::open_split_resilient(&truth_root, &db_path, &device_id)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    let mut guard = ephemeral_cell().lock().unwrap_or_else(|p| p.into_inner());
    *guard = Some(VaultState {
        vault,
        truth_root,
        db_path,
        device_id,
        // 临时会话没有「App 沙盒 Documents / iCloud」概念,固定等于会话根 —— 仅为
        // 满足 VaultState 结构体共用,本 cell 的任何函数都不会拿它做 iCloud 判断。
        docs_dir: session_root,
        data_dir,
    });
    Ok(())
}

/// 采集(图片,Flutter 端已识别好文本):镜像 `api::vault::ingest_image_with_text`,
/// 落临时会话箱。
pub fn ephemeral_ingest_image_with_text(
    name: String,
    bytes: Vec<u8>,
    ocr_text: String,
    confidence: f32,
) -> anyhow::Result<ImportOutcomeDto> {
    with_ephemeral(|state| ingest_image_with_text_core(state, name, bytes, ocr_text, confidence))
}

/// 采集(字节直传):镜像 `api::vault::ingest_bytes`,落临时会话箱。
pub fn ephemeral_ingest_bytes(filename: String, data: Vec<u8>) -> anyhow::Result<ImportOutcomeDto> {
    with_ephemeral(|state| ingest_bytes_core(state, filename, data))
}

/// 预览时间线:镜像 `api::vault::load_archive`,给医生在交付前核对这次代拍收了
/// 什么、分类对不对。
pub fn ephemeral_load_preview() -> anyhow::Result<Vec<TimelineGroupDto>> {
    with_ephemeral(load_archive_core)
}

/// 打包成加密分享件(带拍前同意记录),写进**临时会话箱**的 `shares/`——不是医生
/// 自己的 vault。`consent` 转换见 `From<ConsentDto>`。
pub fn ephemeral_create_share(
    expires_days: i64,
    consent: ConsentDto,
) -> anyhow::Result<ShareResultDto> {
    with_ephemeral(|state| create_share_core(state, expires_days, Some(consent.into())))
}

impl From<ConsentDto> for medme_share::share::ShareConsent {
    fn from(c: ConsentDto) -> Self {
        medme_share::share::ShareConsent {
            utc_ts: c.utc_ts,
            consent_text_version: c.consent_text_version,
            signature_png_base64: c.signature_png_base64,
            method: c.method,
            session_id: c.session_id,
        }
    }
}

/// 即焚:关掉这次会话的 db/日志句柄(`drop`),再整棵删掉它的会话根目录
/// (CAS 原字节 + 事件日志 + db/wal/shm + OCR 文本 + 生成的 share html 全在这棵目录下,
/// 一次 `remove_dir_all` 清干净)。cell 置空。用户取消 / 交付完成 / 路由 dispose 兜底
/// 都调这个,幂等——未开始过会话时是 no-op。
pub fn ephemeral_wipe() -> anyhow::Result<()> {
    let mut guard = ephemeral_cell().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(state) = guard.take() {
        let session_root = state.truth_root.parent().map(|p| p.to_path_buf());
        drop(state); // 显式:先关连接/日志句柄,再删目录
        if let Some(root) = session_root {
            let _ = std::fs::remove_dir_all(&root); // 尽力删除;失败不致命,sweep 兜底
        }
    }
    Ok(())
}

/// 启动时清崩溃残留:遍历 `<cache_dir>` 下所有 `ephemeral-*` 前缀目录并删除。
/// App 启动(`main()`,`RustLib.init()` 之后)调一次。不依赖当前进程是否持有某个
/// cell(上次进程崩溃/被系统杀掉时,`ephemeral_wipe` 根本没机会跑,残留只能靠这个
/// 兜底 + iOS 系统本就可能随时清空 `getTemporaryDirectory()` 双保险)。
pub fn ephemeral_sweep(cache_dir: String) -> anyhow::Result<()> {
    let cache_root = PathBuf::from(cache_dir);
    let entries = match std::fs::read_dir(&cache_root) {
        Ok(e) => e,
        Err(_) => return Ok(()), // 目录不存在等同「没有残留」,不是错误
    };
    for entry in entries.flatten() {
        let is_ephemeral_dir = entry
            .file_type()
            .map(|t| t.is_dir())
            .unwrap_or(false)
            && entry
                .file_name()
                .to_str()
                .is_some_and(|n| n.starts_with(EPHEMERAL_DIR_PREFIX));
        if is_ephemeral_dir {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // 这些测试串行跑同一个进程级 EPHEMERAL cell(和生产代码一样,一次只有一个
    // 活跃会话),不能像多数 Rust 测试那样并发跑;用一把粗互斥锁串行化,避免
    // 相互践踏对方的会话状态。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn begin_ingest_wipe_round_trip() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let cache = tempfile::tempdir().unwrap();
        let cache_dir = cache.path().to_string_lossy().to_string();

        ephemeral_begin(cache_dir.clone()).unwrap();

        // 会话目录应已在 cache_dir 下创建,前缀符合 sweep 的识别规则。
        let session_dirs: Vec<_> = std::fs::read_dir(&cache_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with(EPHEMERAL_DIR_PREFIX))
            })
            .collect();
        assert_eq!(session_dirs.len(), 1, "应恰好建了一个会话目录");

        let outcome =
            ephemeral_ingest_bytes("血常规.txt".into(), b"data".to_vec().repeat(50)).unwrap();
        assert_eq!(outcome.status, "new");

        let preview = ephemeral_load_preview().unwrap();
        assert_eq!(preview.len(), 1, "刚采集的一份应出现在预览时间线里");

        ephemeral_wipe().unwrap();

        // wipe 之后会话目录应已被整棵删除。
        let remaining: Vec<_> = std::fs::read_dir(&cache_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with(EPHEMERAL_DIR_PREFIX))
            })
            .collect();
        assert!(remaining.is_empty(), "wipe 后不应残留会话目录");

        // 未开始会话时调用应报错(不是 panic),wipe 则应是无害 no-op。
        assert!(ephemeral_load_preview().is_err());
        ephemeral_wipe().unwrap(); // 幂等:再 wipe 一次不报错
    }

    #[test]
    fn sweep_removes_crash_leftovers_but_not_other_dirs() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let cache = tempfile::tempdir().unwrap();
        let cache_dir = cache.path().to_string_lossy().to_string();

        // 模拟一次崩溃残留(没走 wipe 就没了的会话目录)+ 一个不相关的目录。
        std::fs::create_dir_all(cache.path().join(format!("{EPHEMERAL_DIR_PREFIX}deadbeef")))
            .unwrap();
        std::fs::create_dir_all(cache.path().join("not-ephemeral")).unwrap();

        ephemeral_sweep(cache_dir).unwrap();

        assert!(!cache.path().join(format!("{EPHEMERAL_DIR_PREFIX}deadbeef")).exists());
        assert!(cache.path().join("not-ephemeral").exists(), "不应误删无关目录");
    }

    #[test]
    fn ephemeral_create_share_embeds_consent() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let cache = tempfile::tempdir().unwrap();
        ephemeral_begin(cache.path().to_string_lossy().to_string()).unwrap();
        ephemeral_ingest_bytes("a.txt".into(), b"hello world".to_vec()).unwrap();

        let consent = ConsentDto {
            utc_ts: "2026-07-22T10:00:00Z".into(),
            consent_text_version: "v1".into(),
            signature_png_base64: Some("iVBORw0KGgo=".into()),
            method: "signature".into(),
            session_id: "sess-test".into(),
        };
        let result = ephemeral_create_share(7, consent).unwrap();
        assert!(result.byte_size > 0);
        assert!(!result.passphrase.is_empty());

        ephemeral_wipe().unwrap();
    }
}
