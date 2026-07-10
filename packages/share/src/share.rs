//! 端到端加密分享(v1.0,零服务器)。
//!
//! 患者在本机把全部病历打包成一份 **自包含加密 HTML**:文件里同时含有(a)用
//! AES-256-GCM 加密后的记录 JSON(base64),(b)一个纯前端查看器。患者把文件存到
//! 自己的云盘或直接发给医生,再 **另行单独** 告知一段 **口令**(=32 字节密钥的
//! base64url)。医生用任意浏览器打开文件、输入口令,浏览器用 Web Crypto 在 **本地**
//! 解密并渲染 —— 全程不经过任何服务器。
//!
//! 互操作要点(Rust 加密 ↔ 浏览器解密必须字节级一致):
//!   - Rust 用 `aes-gcm`(`Aes256Gcm`,128-bit tag,tag 追加在密文尾部)。
//!   - blob 布局:`nonce(12) || ciphertext_with_tag`,整体标准 base64 后内嵌进 HTML。
//!   - 口令 = 32 字节密钥的 URL-safe base64(无填充);显示时按 4 字符 **空格** 分组
//!     便于口述,查看器解码前只去掉空白字符。注意:分组分隔符只能用空格,不能用
//!     "-",因为 "-" 是 base64url 字母表本身的字符,去掉会破坏密钥。
//!   - Web Crypto 的 AES-GCM 同样期望 128-bit tag 追加在密文尾部 —— 与本模块输出一致。

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD as B64URL};
use base64::Engine as _;
use core_model::Vault;
use rand::RngCore;

/// AES-GCM 的固定关联数据(AAD)。绑定后可抵御格式/版本混淆:任何解密端都必须传入
/// 逐字节相同的 AAD 才能解密成功。互操作三处必须一致——Rust 加密、内嵌查看器 JS
/// 解密、`web/hosted-viewer` JS 解密——JS 侧以 `new TextEncoder().encode("medme-share-v1")`
/// 得到相同字节。改动此常量将使旧分享无法解密。
const SHARE_AAD: &[u8] = b"medme-share-v1";

/// 把无填充 base64url 口令按 4 字符分组、空格连接,便于口述/抄写。
/// 查看器解码前会 `replace(/[\s-]/g,'')` 还原,因此分组仅影响显示。
fn group_passphrase(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    chars
        .chunks(4)
        .map(|c| c.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
}

fn fmt_date(d: Option<chrono::DateTime<chrono::Utc>>) -> Option<String> {
    d.map(|x| x.format("%Y-%m-%d").to_string())
}

/// 单个影像检查内嵌完整 DICOM 字节的上限:超过则降级为“锚点切片 PNG + 说明”,
/// 避免单份分享因一叠大序列爆炸(014 §5.3)。
const SHARE_IMAGING_CAP: usize = 40 * 1024 * 1024; // 40 MB / study
/// 整份分享内嵌原始字节(影像+图片)的总上限:一旦累计到顶,后续影像一律降级为
/// PNG + 说明(014 §5.4 自包含 HTML 体积上限保护)。绝不静默截断——每个被降级的
/// 检查都在其卡片留说明,并汇总进 payload.degraded。
const SHARE_TOTAL_CAP: usize = 300 * 1024 * 1024; // 300 MB total

/// 决定某影像检查内嵌方式的结果(便于单测直接断言分档逻辑)。
#[derive(Debug, PartialEq)]
enum ImagingTier {
    /// 内嵌全部切片原始字节 → 浏览器交互式阅片。
    Interactive,
    /// 降级:只内嵌锚点切片 PNG + 说明。`by_total` 区分“单检查超限”还是“总量超限”。
    PngFallback { by_total: bool },
}

/// 纯函数:给定本检查压缩后总字节数、已内嵌总字节数,判定分档。抽出来便于测试。
fn decide_imaging_tier(study_bytes: usize, already_embedded: usize) -> ImagingTier {
    if study_bytes > SHARE_IMAGING_CAP {
        return ImagingTier::PngFallback { by_total: false };
    }
    if already_embedded + study_bytes > SHARE_TOTAL_CAP {
        return ImagingTier::PngFallback { by_total: true };
    }
    ImagingTier::Interactive
}

/// 构建加密分享 HTML。返回 `(html, 分组后的口令, 记录数)`。
pub fn build_encrypted_share(
    v: &Vault,
    expires_days: u32,
) -> Result<(String, String, i64), String> {
    let records = crate::export::gather_records(v)?;
    let profile = pipeline::patient_profile(v).map_err(|e| e.to_string())?;

    let generated = chrono::Utc::now();
    let expires = generated + chrono::Duration::days(expires_days as i64);

    // ── 记录数组 ──
    let mut record_count: i64 = 0;
    let mut records_json: Vec<serde_json::Value> = Vec::new();
    // 已内嵌原始字节累计(影像切片 + 图片),用于整份体积上限判定。
    let mut embedded_bytes: usize = 0;
    // 被降级为 PNG 的影像检查标题(汇总进 payload,避免静默截断)。
    let mut degraded: Vec<String> = Vec::new();
    for rec in &records {
        let doc = &rec.doc;
        let sf = &rec.source_file;
        let title = doc
            .title
            .clone()
            .unwrap_or_else(|| sf.original_name.clone());

        // 仅内嵌 image/* 原件为 data-URI;PDF 不内嵌(仅文字)。
        let mut images: Vec<String> = Vec::new();
        if sf.mime_type.starts_with("image/") {
            let bytes = std::fs::read(v.root_join(&sf.storage_path)).map_err(|e| e.to_string())?;
            embedded_bytes += bytes.len();
            let b64 = B64.encode(&bytes);
            images.push(format!("data:{};base64,{}", sf.mime_type, b64));
        }

        // ── 影像(DICOM):按体积分档内嵌(诊断档 / 关键切片降级)──
        let mut dicom_json = serde_json::Value::Null;
        if sf.mime_type == "application/dicom" {
            // 取该检查的切片清单(已按堆栈顺序);无切片记录时退回文档自身锚点切片。
            let insts = v.imaging_instances(doc.id).map_err(|e| e.to_string())?;
            let ids: Vec<i64> = if insts.is_empty() {
                vec![sf.id]
            } else {
                insts.iter().map(|i| i.source_file_id).collect()
            };
            // 逐张读原始字节(顺序 = 堆栈顺序)。
            let mut slices: Vec<Vec<u8>> = Vec::with_capacity(ids.len());
            for id in &ids {
                if let Some(s) = v.source_file_by_id(*id).map_err(|e| e.to_string())? {
                    let b =
                        std::fs::read(v.root_join(&s.storage_path)).map_err(|e| e.to_string())?;
                    slices.push(b);
                }
            }
            let study_bytes: usize = slices.iter().map(|b| b.len()).sum();
            match decide_imaging_tier(study_bytes, embedded_bytes) {
                ImagingTier::Interactive => {
                    embedded_bytes += study_bytes;
                    let frames: Vec<String> = slices.iter().map(|b| B64.encode(b)).collect();
                    dicom_json = serde_json::json!({
                        "mode": "interactive",
                        "frames": frames,
                        "count": ids.len(),
                    });
                }
                ImagingTier::PngFallback { by_total } => {
                    degraded.push(title.clone());
                    // 锚点切片(第一张)渲成 PNG;不支持的压缩则连 PNG 也没有,只留说明。
                    let png = slices
                        .first()
                        .and_then(|b| dicom::render_png(b).ok())
                        .map(|p| format!("data:image/png;base64,{}", B64.encode(&p)));
                    let note = if by_total {
                        "为控制分享文件体积,本影像未内嵌完整序列(整份已达上限并降级);如需诊断级请当面出示或用托管分享(后续)。".to_string()
                    } else {
                        format!(
                            "完整影像较大未内嵌(约 {} MB,超单检查 {} MB 上限);如需诊断级请当面出示或用托管分享(后续)。",
                            study_bytes / 1024 / 1024,
                            SHARE_IMAGING_CAP / 1024 / 1024
                        )
                    };
                    dicom_json = serde_json::json!({
                        "mode": "png",
                        "png": png,
                        "note": note,
                        "count": ids.len(),
                    });
                }
            }
        }

        records_json.push(serde_json::json!({
            "doc_type": doc.doc_type.as_str(),
            "doc_date": fmt_date(doc.doc_date),
            "doc_date_end": fmt_date(doc.doc_date_end),
            "title": title,
            "text": rec.text,
            "images": images,
            "dicom": dicom_json,
        }));
        record_count += 1;
    }

    if !degraded.is_empty() {
        eprintln!(
            "share: {} 个影像检查因体积上限降级为关键切片:{}",
            degraded.len(),
            degraded.join("、")
        );
    }

    let payload = serde_json::json!({
        "generated": generated.to_rfc3339(),
        "expires": expires.to_rfc3339(),
        "patient": {
            "name": profile.name,
            "gender": profile.gender,
            "age": profile.age,
            "record_count": record_count,
        },
        "records": records_json,
        "degraded": degraded,
    });
    let plaintext = serde_json::to_vec(&payload).map_err(|e| format!("serialize payload: {e}"))?;

    // ── AES-256-GCM 加密 ──
    let mut key_bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut key_bytes);
    let mut nonce_bytes = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);

    let cipher = Aes256Gcm::new_from_slice(&key_bytes).map_err(|e| format!("init cipher: {e}"))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(
            nonce,
            Payload {
                msg: plaintext.as_ref(),
                aad: SHARE_AAD,
            },
        )
        .map_err(|e| format!("encrypt: {e}"))?; // 密文尾部含 16 字节 tag

    // blob = nonce(12) || ciphertext_with_tag,整体标准 base64。
    let mut blob = Vec::with_capacity(12 + ciphertext.len());
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ciphertext);
    let blob_b64 = B64.encode(&blob);

    // 口令 = 密钥的 url-safe base64(无填充);显示时分组。
    let passphrase_raw = B64URL.encode(key_bytes);
    let passphrase_grouped = group_passphrase(&passphrase_raw);

    // dicom-parser 内联进 HTML(约 32KB),保证分享文件自包含、离线可用。
    // 用 replace 在运行时注入而非写死进模板字面量,避免其内容与 Rust 原始字符串
    // 分隔符冲突;先注入解析器,再注入 blob(解析器体量小、注入顺序无副作用)。
    let html = VIEWER_TEMPLATE
        .replace("/*__DICOM_PARSER__*/", DICOM_PARSER_JS)
        .replace("__BLOB__", &blob_b64)
        .replace("__EXPIRES__", &expires.to_rfc3339())
        .replace("__GENERATED__", &generated.to_rfc3339());

    Ok((html, passphrase_grouped, record_count))
}

/// 内联的 dicom-parser(UMD 版,浏览器全局 `dicomParser`)。随源码 vendored 进
/// 仓库,`include_str!` 在编译期嵌入 —— 不依赖 node_modules 即可构建。
const DICOM_PARSER_JS: &str = include_str!("vendor/dicomParser.min.js");

/// 自包含查看器模板。占位符 `__BLOB__` / `__EXPIRES__` / `__GENERATED__` 与
/// `/*__DICOM_PARSER__*/` 用 `str::replace` 注入 —— 避免 `format!` 与内联 JS/CSS
/// 的 `{}` 冲突。无任何外部引用,严格离线可用。
const VIEWER_TEMPLATE: &str = r####"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src data:; connect-src 'none'">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>MedMe 加密病历分享</title>
<style>
  * { box-sizing: border-box; }
  body { font-family: -apple-system, "PingFang SC", "Microsoft YaHei", "Noto Sans CJK SC", "Segoe UI", sans-serif; color: #1e293b; margin: 0; padding: 0; background: #f8fafc; }
  .wrap { max-width: 900px; margin-inline: auto; padding: 24px; }
  /* 口令输入屏 */
  .gate { min-height: 100vh; display: flex; align-items: center; justify-content: center; padding: 24px; }
  .gate-card { background: #fff; border: 1px solid #e2e8f0; border-radius: 16px; padding: 32px; max-width: 420px; width: 100%; box-shadow: 0 8px 30px rgba(15,23,42,.06); }
  .gate-card h1 { font-size: 20px; color: #1d4ed8; margin: 0 0 6px; }
  .gate-card p { font-size: 13px; color: #64748b; line-height: 1.6; margin: 0 0 18px; }
  .gate-card label { display: block; font-size: 12px; font-weight: 600; color: #334155; margin-bottom: 6px; }
  .gate-card input { width: 100%; font-size: 15px; font-family: ui-monospace, SFMono-Regular, Menlo, monospace; padding: 11px 12px; border: 1px solid #cbd5e1; border-radius: 10px; letter-spacing: .5px; }
  .gate-card input:focus { outline: none; border-color: #2563eb; box-shadow: 0 0 0 3px rgba(37,99,235,.15); }
  .gate-card button { width: 100%; margin-top: 14px; font-size: 15px; font-weight: 600; color: #fff; background: #2563eb; border: none; border-radius: 10px; padding: 12px; cursor: pointer; }
  .gate-card button:hover { background: #1d4ed8; }
  .gate-err { color: #be123c; font-size: 13px; margin-top: 12px; min-height: 18px; }
  /* 头部 */
  .doc-header { border-bottom: 2px solid #2563eb; padding-bottom: 12px; margin-bottom: 20px; }
  .doc-header h1 { font-size: 22px; color: #1d4ed8; margin: 0 0 6px; }
  .patient { font-size: 14px; color: #334155; }
  .generated { font-size: 12px; color: #94a3b8; margin-top: 4px; }
  .privacy-note { font-size: 12px; color: #475569; background: #eff6ff; border: 1px solid #dbeafe; border-radius: 10px; padding: 10px 12px; margin-bottom: 20px; line-height: 1.6; }
  /* 记录卡片 */
  .record { background: #fff; border: 1px solid #e2e8f0; border-radius: 12px; padding: 16px 20px; margin-bottom: 16px; page-break-inside: avoid; }
  .record-head { display: flex; align-items: baseline; gap: 10px; flex-wrap: wrap; margin-bottom: 10px; }
  .record-head h2 { font-size: 16px; margin: 0; color: #0f172a; flex: 1; min-width: 120px; }
  .badge { font-size: 11px; font-weight: 700; border-radius: 999px; padding: 2px 10px; }
  .date { font-size: 12px; color: #64748b; font-variant-numeric: tabular-nums; }
  .content { font-size: 15px; line-height: 1.7; color: #334155; }
  .content > * + * { margin-top: 10px; }
  .content table { width: 100%; border-collapse: collapse; font-size: 13px; border: 1px solid #e2e8f0; border-radius: 10px; overflow: hidden; }
  .content thead tr { background: #f8fafc; color: #64748b; font-size: 12px; }
  .content th { text-align: left; font-weight: 600; padding: 7px 12px; border-bottom: 1px solid #e2e8f0; white-space: nowrap; }
  .content td { padding: 6px 12px; font-family: ui-monospace, SFMono-Regular, Menlo, monospace; border-bottom: 1px solid #f1f5f9; white-space: nowrap; }
  .content tr.high td { color: #b45309; }
  .content tr.low td { color: #1d4ed8; }
  .content tr.normal td { color: #334155; }
  .content .section { font-weight: 600; color: #0f172a; padding-top: 2px; }
  .content .label { font-weight: 600; color: #0f172a; }
  .content .para { white-space: pre-wrap; word-break: break-word; }
  /* 处方 */
  .meds { display: flex; flex-direction: column; gap: 8px; }
  .med { display: flex; gap: 12px; background: #ecfdf5; border: 1px solid #d1fae5; border-radius: 12px; padding: 12px; }
  .med .n { width: 26px; height: 26px; border-radius: 8px; background: #d1fae5; color: #047857; display: flex; align-items: center; justify-content: center; flex-shrink: 0; font-weight: 700; font-size: 13px; }
  .med .name { font-weight: 600; color: #1e293b; }
  .med .usage { font-size: 13px; color: #64748b; line-height: 1.6; }
  .meds-label { font-size: 11px; font-family: ui-monospace, monospace; color: #94a3b8; letter-spacing: .15em; text-transform: uppercase; }
  .img { max-width: 100%; max-height: 480px; display: block; margin: 8px 0; border: 1px solid #e2e8f0; border-radius: 8px; }
  /* 影像检查:缩略卡 + 说明 */
  .imaging-card { margin: 10px 0; }
  .imaging-open { display: inline-flex; align-items: center; gap: 8px; cursor: pointer; background: #0f172a; color: #fff; border: none; border-radius: 10px; padding: 10px 16px; font-size: 14px; font-weight: 600; }
  .imaging-open:hover { background: #1e293b; }
  .imaging-open .ico { font-size: 16px; }
  .imaging-meta { font-size: 12px; color: #64748b; margin-top: 6px; line-height: 1.6; }
  .imaging-png { max-width: 100%; max-height: 480px; display: block; margin: 8px 0; background: #000; border: 1px solid #e2e8f0; border-radius: 8px; }
  .imaging-note { font-size: 12px; color: #b45309; background: #fffbeb; border: 1px solid #fde68a; border-radius: 8px; padding: 8px 10px; margin-top: 6px; line-height: 1.6; }
  /* 全屏阅片 overlay(移植桌面端 lightbox 观感)*/
  .dcm-overlay { position: fixed; inset: 0; z-index: 60; background: rgba(0,0,0,.9); display: flex; flex-direction: column; }
  .dcm-bar { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; padding: 8px 12px; background: rgba(0,0,0,.6); color: rgba(255,255,255,.85); }
  .dcm-bar .name { font-family: ui-monospace, Menlo, monospace; font-size: 13px; margin-right: auto; max-width: 40%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .dcm-btn { font-size: 12px; padding: 6px 12px; border-radius: 8px; background: rgba(255,255,255,.1); color: rgba(255,255,255,.85); border: none; cursor: pointer; }
  .dcm-btn:hover { background: rgba(255,255,255,.2); }
  .dcm-doctor { display: inline-flex; align-items: center; gap: 5px; font-size: 11px; font-weight: 600; color: #fcd34d; }
  .dcm-sep { width: 1px; height: 16px; background: rgba(255,255,255,.2); }
  .dcm-idx { font-family: ui-monospace, Menlo, monospace; font-size: 12px; color: rgba(255,255,255,.7); }
  .dcm-hintbar { font-size: 11px; color: rgba(255,255,255,.7); margin-left: auto; }
  .dcm-stage { position: relative; flex: 1; min-height: 0; background: #000; overflow: hidden; }
  .dcm-stage canvas { display: block; touch-action: none; }
  .dcm-notice { position: absolute; top: 16px; left: 50%; transform: translateX(-50%); background: rgba(0,0,0,.75); color: rgba(255,255,255,.9); font-size: 12px; padding: 8px 16px; border-radius: 8px; max-width: 80%; text-align: center; pointer-events: none; }
  .dcm-floathint { position: absolute; bottom: 24px; left: 50%; transform: translateX(-50%); background: rgba(0,0,0,.75); color: #fff; font-size: 14px; padding: 10px 20px; border-radius: 999px; pointer-events: none; }
  .statement { text-align: center; font-size: 11px; color: #94a3b8; margin-top: 24px; padding-top: 12px; border-top: 1px solid #e2e8f0; }
  .expired { max-width: 480px; margin: 80px auto; text-align: center; background: #fff; border: 1px solid #fecdd3; border-radius: 16px; padding: 40px 28px; }
  .expired h1 { font-size: 18px; color: #be123c; margin: 0 0 10px; }
  .expired p { font-size: 14px; color: #64748b; line-height: 1.7; margin: 0; }
  @media print {
    body { background: #fff; }
    .record { border: 1px solid #cbd5e1; box-shadow: none; }
    .privacy-note { background: #fff; }
    @page { margin: 16mm 14mm; }
  }
</style>
</head>
<body>
<div id="gate" class="gate">
  <div class="gate-card">
    <h1>MedMe 加密病历</h1>
    <p>这份文件已端到端加密。请输入本人另行告知的<b>口令</b>,浏览器将在本地解密并显示病历,数据不会上传任何服务器。</p>
    <label for="pw">口令</label>
    <input id="pw" type="password" autocomplete="off" spellcheck="false" placeholder="粘贴或输入口令">
    <button id="go" type="button">解密查看</button>
    <div id="err" class="gate-err"></div>
  </div>
</div>
<div id="app" class="wrap" style="display:none"></div>

<script>
/*__DICOM_PARSER__*/
</script>
<script>
const EMBEDDED_BLOB = "__BLOB__";

const TYPE_LABEL = { lab_report:"化验", imaging_report:"检查", discharge_summary:"出院", prescription:"处方", clinical_note:"病历", pathology:"病理", surgery:"手术", other:"其他", unknown:"未分类" };
// bg | color(与桌面端 TYPE_BADGE 一致)
const TYPE_BADGE = {
  lab_report:      ["#eff6ff","#1d4ed8"],
  imaging_report:  ["#fffbeb","#b45309"],
  discharge_summary:["#eef2ff","#4338ca"],
  prescription:    ["#ecfdf5","#047857"],
  clinical_note:   ["#f0f9ff","#0369a1"],
  pathology:       ["#fff1f2","#be123c"],
  surgery:         ["#faf5ff","#7e22ce"],
  other:           ["#f1f5f9","#475569"],
  unknown:         ["#f1f5f9","#475569"],
};

function b64ToBytes(b64) {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}
function b64urlToBytes(s) {
  let t = s.replace(/-/g, "+").replace(/_/g, "/");
  while (t.length % 4) t += "=";
  return b64ToBytes(t);
}
function esc(s) {
  return String(s).replace(/[&<>"']/g, c =>
    ({ "&":"&amp;", "<":"&lt;", ">":"&gt;", '"':"&quot;", "'":"&#39;" }[c]));
}

// ── 内容解析(移植 ReportContent.tsx)──
function splitCells(line) { return line.trim().split(/\s{2,}|\t/).filter(c => c.length > 0); }
function isTableHeader(line) {
  const keys = ["项目","结果","单位","参考","提示","名称","缩写"];
  return keys.filter(k => line.includes(k)).length >= 2 && splitCells(line).length >= 3;
}
function isDataRow(line) { return splitCells(line).length >= 3 && /\d/.test(line); }
function rowStatus(cells) {
  const j = cells.join(" ");
  if (cells.includes("↑") || /↑|偏高|升高/.test(j)) return "high";
  if (cells.includes("↓") || /↓|偏低|降低|减低/.test(j)) return "low";
  if (/正常/.test(j)) return "normal";
  return "";
}
function parseBlocks(text) {
  const lines = text.split(/\r?\n/);
  const blocks = [];
  let i = 0;
  while (i < lines.length) {
    const trimmed = lines[i].trim();
    if (!trimmed) { i++; continue; }
    if (isTableHeader(trimmed) || isDataRow(trimmed)) {
      const start = i;
      const header = isTableHeader(trimmed) ? splitCells(trimmed) : null;
      if (header) i++;
      const rows = [];
      while (i < lines.length && lines[i].trim() && isDataRow(lines[i])) {
        rows.push(splitCells(lines[i])); i++;
      }
      if (rows.length >= 2) { blocks.push({ kind:"table", header, rows }); continue; }
      i = start;
    }
    if (/^[【[].+[】\]]$/.test(trimmed) || (trimmed.length <= 14 && /[:：]$/.test(trimmed))) {
      blocks.push({ kind:"section", text: trimmed });
    } else {
      blocks.push({ kind:"para", text: lines[i] });
    }
    i++;
  }
  return blocks;
}
const LABEL_RE = /^([一-龥A-Za-z]{2,10})([:：])(.*)$/;
function renderPara(text) {
  const t = text.replace(/\s+$/, "");
  const m = t.match(LABEL_RE);
  if (m && m[3].trim().length > 0) {
    return '<div class="para"><span class="label">' + esc(m[1]) + esc(m[2]) + "</span>" + esc(m[3]) + "</div>";
  }
  return '<div class="para">' + esc(text) + "</div>";
}
function renderBlocks(blocks) {
  let html = "";
  for (const b of blocks) {
    if (b.kind === "table") {
      const cols = Math.max(b.header ? b.header.length : 0, ...b.rows.map(r => r.length));
      html += '<div style="overflow-x:auto"><table>';
      if (b.header) {
        html += "<thead><tr>";
        for (const h of b.header) html += "<th>" + esc(h) + "</th>";
        html += "</tr></thead>";
      }
      html += "<tbody>";
      for (const r of b.rows) {
        const st = rowStatus(r);
        html += '<tr class="' + st + '">';
        for (let c = 0; c < cols; c++) html += "<td>" + esc(r[c] || "") + "</td>";
        html += "</tr>";
      }
      html += "</tbody></table></div>";
    } else if (b.kind === "section") {
      html += '<div class="section">' + esc(b.text) + "</div>";
    } else {
      html += renderPara(b.text);
    }
  }
  return html;
}
// ── 处方:用药清单(移植 parseMeds)──
function parseMeds(text) {
  const lines = text.split(/\r?\n/);
  const meds = [], intro = [], footer = [];
  let cur = null, started = false, ended = false;
  for (const raw of lines) {
    const line = raw.trim();
    const numbered = line.match(/^(\d+)\s*[.、)]\s*(.+)/);
    if (numbered) {
      started = true; ended = false;
      if (cur) meds.push(cur);
      cur = { name: numbered[2].trim(), usage: [] };
      continue;
    }
    if (/^(医师|药师|审核|备注|Rp\.?|处方)/.test(line)) {
      if (cur) { meds.push(cur); cur = null; }
      if (started) ended = true;
      if (line && !/^Rp\.?$/.test(line)) { if (started) footer.push(line); else intro.push(line); }
      continue;
    }
    if (cur && line) { cur.usage.push(line); continue; }
    if (line) { if (!started) intro.push(line); else if (ended) footer.push(line); }
  }
  if (cur) meds.push(cur);
  return meds.length ? { intro, meds, footer } : null;
}
function renderContent(text, docType) {
  if (!text || !text.trim()) return '<div style="color:#94a3b8;font-size:14px">无文本内容。</div>';
  if (docType === "prescription") {
    const p = parseMeds(text);
    if (p) {
      let html = "";
      if (p.intro.length) html += p.intro.map(renderPara).join("");
      html += '<div class="meds-label">用药</div><div class="meds">';
      p.meds.forEach((m, i) => {
        html += '<div class="med"><div class="n">' + (i + 1) + '</div><div><div class="name">' + esc(m.name) + "</div>";
        html += m.usage.map(u => '<div class="usage">' + esc(u) + "</div>").join("");
        html += "</div></div>";
      });
      html += "</div>";
      if (p.footer.length) html += '<div style="color:#64748b;font-size:13px">' + p.footer.map(renderPara).join("") + "</div>";
      return html;
    }
  }
  return renderBlocks(parseBlocks(text));
}

// ══ 交互式 DICOM 阅片器(移植 DicomViewer.tsx 的解码逻辑到纯 JS + canvas)══
// 纯 dicom-parser + canvas,无 Cornerstone / worker / WASM。按需解码 + 有界缓存,
// 几百帧也只常驻几十 MB(014 §5 大数据顾虑)。
const DCM_TS_UNCOMPRESSED = new Set(["1.2.840.10008.1.2","1.2.840.10008.1.2.1","1.2.840.10008.1.2.1.99"]);
const DCM_TS_JPEG_BASELINE = new Set(["1.2.840.10008.1.2.4.50","1.2.840.10008.1.2.4.51"]);
const DCM_PRESETS = [
  { label:"默认", center:null, width:null },
  { label:"脑窗", center:40, width:80 },
  { label:"骨窗", center:500, width:2000 },
  { label:"肺窗", center:-600, width:1500 },
  { label:"软组织", center:40, width:400 },
];
const DCM_CACHE_MAX = 7; // 当前帧 + 邻近几帧;超出按离当前最远淘汰。

function dcmNum(ds, tag, def) { const v = ds.uint16(tag); return v === undefined ? def : v; }
function dcmFloat(ds, tag) { const v = ds.floatString(tag); return (v === undefined || Number.isNaN(v)) ? null : v; }

// 解析每张切片 → 展平成 frames[](NumberOfFrames>1 的多帧展开)。解析失败的切片跳过。
function dcmBuildFrames(slices) {
  const frames = [];
  for (const bytes of slices) {
    let ds;
    try { ds = dicomParser.parseDicom(bytes); } catch (e) { continue; }
    const rows = dcmNum(ds, "x00280010", 0);
    const columns = dcmNum(ds, "x00280011", 0);
    if (!rows || !columns) continue;
    const bitsAllocated = dcmNum(ds, "x00280100", 16);
    const pixelRepresentation = dcmNum(ds, "x00280103", 0);
    const samplesPerPixel = dcmNum(ds, "x00280002", 1);
    const planarConfiguration = dcmNum(ds, "x00280006", 0);
    const photometric = (ds.string("x00280004") || "MONOCHROME2").trim().toUpperCase();
    const transferSyntax = (ds.string("x00020010") || "1.2.840.10008.1.2").trim();
    const nFrames = parseInt(ds.string("x00280008") || "1", 10) || 1;
    const defaultCenter = dcmFloat(ds, "x00281050");
    const defaultWidth = dcmFloat(ds, "x00281051");
    for (let f = 0; f < nFrames; f++) {
      frames.push({ dataSet: ds, frameIndex: f, rows, columns, bitsAllocated, pixelRepresentation,
        samplesPerPixel, planarConfiguration, photometric, invert: photometric === "MONOCHROME1",
        color: samplesPerPixel >= 3, transferSyntax, defaultCenter, defaultWidth });
    }
  }
  return frames;
}

// 未压缩灰度:读原始像素 → modality rescale(v = raw*slope + intercept)。
function dcmReadGray(fm) {
  const ds = fm.dataSet;
  const pd = ds.elements.x7fe00010;
  const byteArray = ds.byteArray;
  const bytesPerPixel = fm.bitsAllocated <= 8 ? 1 : 2;
  const pxCount = fm.rows * fm.columns;
  const frameLength = pxCount * bytesPerPixel;
  const absStart = byteArray.byteOffset + pd.dataOffset + fm.frameIndex * frameLength;
  const buf = byteArray.buffer.slice(absStart, absStart + frameLength); // 对齐副本
  const slope = dcmFloat(ds, "x00281053") ?? 1;
  const intercept = dcmFloat(ds, "x00281052") ?? 0;
  const out = new Float32Array(pxCount);
  if (bytesPerPixel === 1) {
    const raw = new Uint8Array(buf);
    for (let i = 0; i < pxCount; i++) out[i] = raw[i] * slope + intercept;
  } else if (fm.pixelRepresentation === 1) {
    const raw = new Int16Array(buf);
    for (let i = 0; i < pxCount; i++) out[i] = raw[i] * slope + intercept;
  } else {
    const raw = new Uint16Array(buf);
    for (let i = 0; i < pxCount; i++) out[i] = raw[i] * slope + intercept;
  }
  return out;
}

// 未压缩彩色(RGB)→ ImageData。处理 PlanarConfiguration 0 交错 / 1 平面。
function dcmReadColor(fm) {
  const ds = fm.dataSet;
  const pd = ds.elements.x7fe00010;
  const byteArray = ds.byteArray;
  const pxCount = fm.rows * fm.columns;
  const frameLength = pxCount * 3;
  const absStart = byteArray.byteOffset + pd.dataOffset + fm.frameIndex * frameLength;
  const src = new Uint8Array(byteArray.buffer, absStart, frameLength);
  const img = new ImageData(fm.columns, fm.rows);
  const d = img.data;
  if (fm.planarConfiguration === 1) {
    const plane = pxCount;
    for (let i = 0; i < pxCount; i++) { d[i*4]=src[i]; d[i*4+1]=src[plane+i]; d[i*4+2]=src[2*plane+i]; d[i*4+3]=255; }
  } else {
    for (let i = 0; i < pxCount; i++) { d[i*4]=src[i*3]; d[i*4+1]=src[i*3+1]; d[i*4+2]=src[i*3+2]; d[i*4+3]=255; }
  }
  return img;
}

async function dcmDecodeFrame(fm) {
  try {
    const ts = fm.transferSyntax;
    if (DCM_TS_UNCOMPRESSED.has(ts)) {
      if (fm.color) return { kind:"rgba", data: dcmReadColor(fm) };
      return { kind:"gray", values: dcmReadGray(fm), rows: fm.rows, cols: fm.columns, invert: fm.invert };
    }
    if (DCM_TS_JPEG_BASELINE.has(ts)) {
      const pd = fm.dataSet.elements.x7fe00010;
      // 空 BOT(如超声动态多帧)→ 按 fragment 下标读;有 BOT → 按帧读。
      const bot = pd.basicOffsetTable;
      const encoded = (bot && bot.length > 0)
        ? dicomParser.readEncapsulatedImageFrame(fm.dataSet, pd, fm.frameIndex)
        : dicomParser.readEncapsulatedPixelDataFromFragments(fm.dataSet, pd, fm.frameIndex);
      const copy = encoded.slice();
      const blob = new Blob([copy], { type: "image/jpeg" });
      const bitmap = await createImageBitmap(blob);
      return { kind:"bitmap", bitmap, rows: bitmap.height, cols: bitmap.width };
    }
    return { kind:"unsupported" }; // JPEG2000 / JPEG-LS / RLE 等此轻量器不解
  } catch (e) {
    console.error("DICOM 帧解码失败", e);
    return { kind:"error" };
  }
}

function dcmDefaultWindow(fm, dec) {
  if (fm.defaultCenter != null && fm.defaultWidth != null && fm.defaultWidth > 0)
    return { center: fm.defaultCenter, width: fm.defaultWidth };
  if (dec.kind === "gray") {
    let mn = Infinity, mx = -Infinity;
    const v = dec.values;
    for (let i = 0; i < v.length; i++) { if (v[i] < mn) mn = v[i]; if (v[i] > mx) mx = v[i]; }
    if (!Number.isFinite(mn) || !Number.isFinite(mx) || mx <= mn) return { center: 128, width: 256 };
    return { center: (mn + mx) / 2, width: mx - mn };
  }
  return { center: 128, width: 256 };
}

// 打开一个全屏阅片 overlay。slices: Uint8Array[]。返回后自动释放。
function openDicomViewer(slices, name) {
  const frames = dcmBuildFrames(slices);
  const overlay = document.createElement("div");
  overlay.className = "dcm-overlay";
  overlay.innerHTML =
    '<div class="dcm-bar">' +
      '<span class="name"></span>' +
      '<button class="dcm-btn" data-z="in">放大</button>' +
      '<button class="dcm-btn" data-z="out">缩小</button>' +
      '<span class="dcm-sep"></span>' +
      '<span class="dcm-doctor" title="窗宽窗位:医生调明暗看不同组织的专业工具">🩺 医生 · 窗位</span>' +
      DCM_PRESETS.map((p,i)=>'<button class="dcm-btn" data-p="'+i+'">'+esc(p.label)+'</button>').join("") +
      '<span class="dcm-sep"></span>' +
      '<button class="dcm-btn" data-reset="1">重置</button>' +
      '<span class="dcm-idx"></span>' +
      '<span class="dcm-hintbar"></span>' +
      '<button class="dcm-btn" data-close="1">关闭 · ESC</button>' +
    '</div>' +
    '<div class="dcm-stage"><canvas></canvas>' +
      '<div class="dcm-notice" style="display:none"></div>' +
      '<div class="dcm-floathint" style="display:none"></div>' +
    '</div>';
  overlay.querySelector(".name").textContent = name || "";
  document.body.appendChild(overlay);

  const canvas = overlay.querySelector("canvas");
  const stage = overlay.querySelector(".dcm-stage");
  const idxEl = overlay.querySelector(".dcm-idx");
  const hintBar = overlay.querySelector(".dcm-hintbar");
  const noticeEl = overlay.querySelector(".dcm-notice");
  const floatHint = overlay.querySelector(".dcm-floathint");

  const cache = new Map();
  let drawToken = 0;
  let win = { center: 40, width: 400 };
  let view = { zoom: 1, panX: 0, panY: 0 };
  let curIdx = 0;
  let drag = null;
  const total = frames.length;

  hintBar.textContent = total > 1 ? "↕ 滚轮翻看每一层 · 左键调明暗 · Ctrl+滚轮缩放" : "左键调明暗 · Ctrl+滚轮缩放";

  function setNotice(t) { if (t) { noticeEl.textContent = t; noticeEl.style.display = "block"; } else noticeEl.style.display = "none"; }
  function setIdx() { idxEl.textContent = total > 1 ? ("第 " + (curIdx+1) + " / 共 " + total + " 张") : ""; }

  function paint(dec) {
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const cw = stage.clientWidth, ch = stage.clientHeight;
    const dpr = window.devicePixelRatio || 1;
    if (canvas.width !== Math.round(cw*dpr) || canvas.height !== Math.round(ch*dpr)) {
      canvas.width = Math.round(cw*dpr); canvas.height = Math.round(ch*dpr);
      canvas.style.width = cw+"px"; canvas.style.height = ch+"px";
    }
    ctx.setTransform(dpr,0,0,dpr,0,0);
    ctx.fillStyle = "#000"; ctx.fillRect(0,0,cw,ch);
    if (!dec || dec.kind === "unsupported" || dec.kind === "error") return;
    let srcCols, srcRows, source;
    if (dec.kind === "bitmap") { srcCols = dec.cols; srcRows = dec.rows; source = dec.bitmap; }
    else if (dec.kind === "rgba") {
      srcCols = dec.data.width; srcRows = dec.data.height;
      const off = document.createElement("canvas"); off.width = srcCols; off.height = srcRows;
      off.getContext("2d").putImageData(dec.data, 0, 0); source = off;
    } else {
      srcCols = dec.cols; srcRows = dec.rows;
      const w = win.width <= 0 ? 1 : win.width; const low = win.center - w/2;
      const img = new ImageData(srcCols, srcRows); const d = img.data; const vals = dec.values;
      for (let i = 0; i < vals.length; i++) {
        let out = ((vals[i]-low)/w)*255; out = out<0?0:out>255?255:out;
        if (dec.invert) out = 255-out;
        const o = i*4; d[o]=d[o+1]=d[o+2]=out; d[o+3]=255;
      }
      const off = document.createElement("canvas"); off.width = srcCols; off.height = srcRows;
      off.getContext("2d").putImageData(img, 0, 0); source = off;
    }
    const fit = Math.min(cw/srcCols, ch/srcRows);
    const dw = srcCols*fit*view.zoom, dh = srcRows*fit*view.zoom;
    const dx = (cw-dw)/2 + view.panX, dy = (ch-dh)/2 + view.panY;
    ctx.imageSmoothingEnabled = true;
    ctx.drawImage(source, dx, dy, dw, dh);
  }

  function evict(current) {
    while (cache.size > DCM_CACHE_MAX) {
      let far = -1, farDist = -1;
      for (const k of cache.keys()) { const dist = Math.abs(k-current); if (dist > farDist) { farDist = dist; far = k; } }
      if (far < 0) break;
      const ent = cache.get(far); if (ent && ent.kind === "bitmap") ent.bitmap.close();
      cache.delete(far);
    }
  }
  function redraw() { const dec = cache.get(curIdx); if (dec) paint(dec); }
  function prefetch(idx) {
    for (const j of [idx+1, idx-1]) {
      if (j < 0 || j >= frames.length || cache.has(j)) continue;
      dcmDecodeFrame(frames[j]).then(d => {
        if (cache.has(j)) { if (d.kind === "bitmap") d.bitmap.close(); return; }
        cache.set(j, d); evict(curIdx);
      });
    }
  }
  async function showFrame(idx, resetWindow) {
    if (idx < 0 || idx >= frames.length) return;
    curIdx = idx; setIdx(); setNotice(null);
    const token = ++drawToken;
    const fm = frames[idx];
    let dec = cache.get(idx);
    if (!dec) {
      dec = await dcmDecodeFrame(fm);
      if (token !== drawToken) { if (dec.kind === "bitmap") dec.bitmap.close(); return; }
      cache.set(idx, dec); evict(idx);
    }
    if (token !== drawToken) return;
    if (dec.kind === "unsupported") setNotice("此压缩格式暂不支持交互查看(见卡片关键切片)");
    if (dec.kind === "error") setNotice("影像加载失败");
    if (resetWindow) win = dcmDefaultWindow(fm, dec);
    paint(dec); prefetch(idx);
  }

  // 交互:滚轮翻层 / Ctrl+滚轮缩放;左键调窗位(灰度)否则平移;右键缩放;中键平移。
  function onWheel(e) {
    e.preventDefault();
    if (e.ctrlKey) { const f = e.deltaY>0?0.9:1.1; view.zoom = Math.min(20, Math.max(0.2, view.zoom*f)); redraw(); return; }
    if (floatHint) floatHint.style.display = "none";
    const next = curIdx + (e.deltaY>0?1:-1);
    if (next < 0 || next >= frames.length) return;
    showFrame(next);
  }
  function onDown(e) { drag = { button: e.button, x: e.clientX, y: e.clientY }; }
  function onMove(e) {
    if (!drag) return;
    const dx = e.clientX-drag.x, dy = e.clientY-drag.y; drag.x = e.clientX; drag.y = e.clientY;
    const dec = cache.get(curIdx);
    if (drag.button === 0) {
      if (dec && dec.kind === "gray") { win.width = Math.max(1, win.width + dx*2); win.center = win.center + dy*2; redraw(); }
      else { view.panX += dx; view.panY += dy; redraw(); }
    } else if (drag.button === 2) { const f = Math.exp(-dy*0.005); view.zoom = Math.min(20, Math.max(0.2, view.zoom*f)); redraw(); }
    else { view.panX += dx; view.panY += dy; redraw(); }
  }
  function onUp() { drag = null; }
  function applyPreset(i) {
    const p = DCM_PRESETS[i]; const fm = frames[curIdx]; const dec = cache.get(curIdx);
    if (!fm || !dec) return;
    win = (p.center == null || p.width == null) ? dcmDefaultWindow(fm, dec) : { center: p.center, width: p.width };
    redraw();
  }
  function reset() {
    view = { zoom: 1, panX: 0, panY: 0 };
    const fm = frames[curIdx]; const dec = cache.get(curIdx);
    if (fm && dec) win = dcmDefaultWindow(fm, dec);
    redraw();
  }

  canvas.addEventListener("wheel", onWheel, { passive: false });
  canvas.addEventListener("mousedown", onDown);
  canvas.addEventListener("mousemove", onMove);
  canvas.addEventListener("mouseup", onUp);
  canvas.addEventListener("mouseleave", onUp);
  canvas.addEventListener("contextmenu", e => e.preventDefault());
  const ro = new ResizeObserver(() => redraw()); ro.observe(stage);

  overlay.addEventListener("click", e => {
    const t = e.target;
    if (t.dataset && t.dataset.close) { close(); return; }
    if (t.dataset && t.dataset.reset) { reset(); return; }
    if (t.dataset && t.dataset.z) { const f = t.dataset.z === "in" ? 1.2 : 1/1.2; view.zoom = Math.min(20, Math.max(0.2, view.zoom*f)); redraw(); return; }
    if (t.dataset && t.dataset.p != null && t.dataset.p !== "") { applyPreset(parseInt(t.dataset.p,10)); return; }
  });
  function onKey(e) { if (e.key === "Escape") close(); }
  document.addEventListener("keydown", onKey);

  function close() {
    drawToken++;
    ro.disconnect();
    document.removeEventListener("keydown", onKey);
    for (const ent of cache.values()) if (ent.kind === "bitmap") ent.bitmap.close();
    cache.clear();
    overlay.remove();
  }

  setIdx();
  if (frames.length === 0) { setNotice("影像加载失败"); return; }
  if (total > 1) { floatHint.textContent = "↕ 滚轮上下翻看每一层(共 " + total + " 张)"; floatHint.style.display = "block";
    setTimeout(() => { floatHint.style.display = "none"; }, 4500); }
  showFrame(0, true);
}

function render(payload) {
  const app = document.getElementById("app");
  const p = payload.patient || {};
  const parts = [];
  if (p.name) parts.push(esc(p.name));
  if (p.gender) parts.push(esc(p.gender));
  if (p.age) parts.push(esc(p.age) + "岁");
  const patientLine = parts.length ? parts.join(" · ") : "(未识别到患者基本信息)";
  const gen = (payload.generated || "").slice(0, 10);

  let html = '<header class="doc-header"><h1>MedMe 医我 · 加密病历分享</h1>';
  html += '<div class="patient">' + patientLine + "</div>";
  html += '<div class="generated">生成时间:' + esc(gen) + " · 共 " + (p.record_count || 0) + " 份记录</div></header>";
  html += '<div class="privacy-note">本页由 MedMe 端到端加密分享生成,数据在您的浏览器本地解密,未上传任何服务器。不构成医疗建议,以原件为准。</div>';

  // 复阅期限:仅为分享方给出的非强制提醒,绝不阻断查看(文件持有者始终可解密)。
  const adviseUntil = (payload.expires || "").slice(0, 10);
  if (adviseUntil) {
    const past = payload.expires && Date.now() > Date.parse(payload.expires);
    const msg = past
      ? '分享方建议的复阅期限(' + esc(adviseUntil) + ')已过 · 文件仍可正常查看,如需最新记录请自行向本人确认。'
      : '分享方建议的复阅期限:' + esc(adviseUntil) + '(仅为提醒,过后文件仍可查看,请自行向本人确认是否为最新)。';
    html += '<div style="margin:0 0 14px;padding:10px 14px;background:#fffbeb;border:1px solid #fde68a;border-radius:10px;color:#92400e;font-size:13px;line-height:1.6">' + msg + '</div>';
  }

  const records = payload.records || [];
  for (let ri = 0; ri < records.length; ri++) {
    const r = records[ri];
    const type = r.doc_type || "unknown";
    const label = TYPE_LABEL[type] || TYPE_LABEL.unknown;
    const bc = TYPE_BADGE[type] || TYPE_BADGE.unknown;
    const title = r.title || label;
    let dateStr = "无日期";
    if (r.doc_date && r.doc_date_end && r.doc_date !== r.doc_date_end) dateStr = r.doc_date + " → " + r.doc_date_end;
    else if (r.doc_date) dateStr = r.doc_date;

    html += '<section class="record"><div class="record-head">';
    html += '<span class="badge" style="background:' + bc[0] + ";color:" + bc[1] + '">' + esc(label) + "</span>";
    html += "<h2>" + esc(title) + '</h2><span class="date">' + esc(dateStr) + "</span></div>";
    html += '<div class="content">' + renderContent(r.text || "", type);
    for (const img of (r.images || [])) {
      if (typeof img === "string" && img.startsWith("data:image/")) html += '<img class="img" src="' + img + '" alt="原件">';
    }
    // 影像检查:诊断档(交互阅片)或降级档(关键切片 PNG + 说明)。
    const dcm = r.dicom;
    if (dcm && dcm.mode === "interactive") {
      const n = dcm.count || (dcm.frames ? dcm.frames.length : 0);
      html += '<div class="imaging-card">';
      html += '<button class="imaging-open" data-dcm="' + ri + '"><span class="ico">🎞</span> 打开影像阅片器' + (n > 1 ? "(共 " + n + " 张,可滚轮翻层)" : "") + '</button>';
      html += '<div class="imaging-meta">诊断档:完整序列已端到端加密内嵌 · 滚轮翻层 · 左键调窗位 · 窗宽窗位预设</div>';
      html += '</div>';
    } else if (dcm && dcm.mode === "png") {
      html += '<div class="imaging-card">';
      if (dcm.png) html += '<img class="imaging-png" src="' + dcm.png + '" alt="影像关键切片">';
      if (dcm.note) html += '<div class="imaging-note">' + esc(dcm.note) + '</div>';
      html += '</div>';
    }
    html += "</div></section>";
  }
  html += '<footer class="statement">本页由 MedMe 端到端加密分享生成 · 数据以原件为准 · 不构成医疗建议</footer>';
  app.innerHTML = html;
  document.getElementById("gate").style.display = "none";
  app.style.display = "block";

  // 阅片按钮:点开时才把 base64 帧解成 Uint8Array 交给查看器(关闭即释放,内存有界)。
  app.querySelectorAll("[data-dcm]").forEach(btn => {
    btn.addEventListener("click", () => {
      const rec = records[parseInt(btn.getAttribute("data-dcm"), 10)];
      if (!rec || !rec.dicom || !rec.dicom.frames) return;
      const slices = rec.dicom.frames.map(f => b64ToBytes(f));
      openDicomViewer(slices, rec.title || "影像检查");
    });
  });
}

async function decryptAndRender(passphrase) {
  const blob = b64ToBytes(EMBEDDED_BLOB);
  const iv = blob.slice(0, 12);
  const data = blob.slice(12);
  // 仅去空白/换行还原分组;不可去 "-",因为它是 base64url 字母表的一部分。
  const keyBytes = b64urlToBytes(passphrase.replace(/\s+/g, ""));
  const key = await crypto.subtle.importKey("raw", keyBytes, { name: "AES-GCM" }, false, ["decrypt"]);
  // AAD 必须与 Rust 加密端 SHARE_AAD 逐字节一致(ASCII "medme-share-v1")。
  const aad = new TextEncoder().encode("medme-share-v1");
  const pt = await crypto.subtle.decrypt({ name: "AES-GCM", iv, additionalData: aad }, key, data); // 口令错误则抛异常
  const payload = JSON.parse(new TextDecoder().decode(pt));
  // 注:expires 只是分享方建议的“复阅期限”,是一条礼貌提醒——文件本身不会失效、不会自毁,
  // 持有文件与口令者始终可离线解密。渲染时以非强制横幅展示,绝不阻断查看。
  render(payload);
}

const pw = document.getElementById("pw");
const errEl = document.getElementById("err");
async function submit() {
  errEl.textContent = "";
  const val = pw.value.trim();
  if (!val) { errEl.textContent = "请输入口令。"; return; }
  try {
    await decryptAndRender(val);
  } catch (e) {
    errEl.textContent = "口令错误,无法解密。请核对本人告知的口令。";
  }
}
document.getElementById("go").addEventListener("click", submit);
pw.addEventListener("keydown", e => { if (e.key === "Enter") submit(); });
pw.focus();
</script>
</body>
</html>
"####;

#[cfg(test)]
mod tests {
    use super::*;
    use aes_gcm::aead::Aead;

    #[test]
    fn build_share_produces_valid_html_and_key() {
        use core_model::{DocType, NewDocument, NewOcr, OcrBackendKind};
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let imp = vault.import("血常规.txt", "text/plain", b"data").unwrap();
        let doc = vault
            .add_document(NewDocument {
                source_file_id: imp.source_file.id,
                doc_type: DocType::LabReport,
                doc_date: Some(chrono::Utc::now()),
                doc_date_end: None,
                title: Some("血常规报告".into()),
                language: Some("zh".into()),
                page_count: 1,
            })
            .unwrap();
        vault
            .add_ocr(NewOcr {
                document_id: doc.id,
                page_no: 1,
                backend: OcrBackendKind::Native,
                model_version: "text-layer".into(),
                text: "白细胞 10.5".into(),
                confidence: None,
            })
            .unwrap();

        let (html, pass, n) = build_encrypted_share(&vault, 5).unwrap();
        assert_eq!(n, 1);
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("EMBEDDED_BLOB = \""));
        assert!(!html.contains("__BLOB__")); // 占位符已全部替换
        assert!(!html.contains("__EXPIRES__"));

        // 口令去空白后应能 base64url 解回 32 字节密钥。
        let stripped: String = pass.chars().filter(|c| !c.is_whitespace()).collect();
        let key = B64URL.decode(stripped).unwrap();
        assert_eq!(key.len(), 32);

        // 提取内嵌 blob → 用该密钥解密 → 应还原出合法 payload JSON(与浏览器查看器同路径)。
        let start = html.find("EMBEDDED_BLOB = \"").unwrap() + "EMBEDDED_BLOB = \"".len();
        let end = html[start..].find('"').unwrap() + start;
        let blob = B64.decode(&html[start..end]).unwrap();
        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        let pt = cipher
            .decrypt(
                Nonce::from_slice(&blob[..12]),
                Payload {
                    msg: &blob[12..],
                    aad: SHARE_AAD,
                },
            )
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&pt).unwrap();
        assert_eq!(payload["records"].as_array().unwrap().len(), 1);
        assert_eq!(payload["records"][0]["doc_type"], "lab_report");
        assert_eq!(payload["patient"]["record_count"], 1);
        assert!(payload["expires"].is_string());
    }

    #[test]
    fn imaging_tier_thresholds() {
        // 小检查 → 内嵌全字节(交互)。
        assert_eq!(
            decide_imaging_tier(10 * 1024 * 1024, 0),
            ImagingTier::Interactive
        );
        // 单检查超上限 → PNG(by_total=false)。
        assert_eq!(
            decide_imaging_tier(SHARE_IMAGING_CAP + 1, 0),
            ImagingTier::PngFallback { by_total: false }
        );
        // 本身不超,但叠加已内嵌超总上限 → PNG(by_total=true)。
        assert_eq!(
            decide_imaging_tier(10 * 1024 * 1024, SHARE_TOTAL_CAP),
            ImagingTier::PngFallback { by_total: true }
        );
    }

    #[test]
    fn share_embeds_small_dicom_study_bytes() {
        // 小的单张 DICOM 检查 → payload 里应带交互档:mode=interactive + 原始字节帧;
        // HTML 里应含内联解析器与查看器入口(自包含、离线可交互)。
        use core_model::{DocType, NewDocument};
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let dcm = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/demo-dataset/dicom/CT_small.dcm"
        ))
        .unwrap();
        let imp = vault
            .import("CT_small.dcm", "application/dicom", &dcm)
            .unwrap();
        vault
            .add_document(NewDocument {
                source_file_id: imp.source_file.id,
                doc_type: DocType::ImagingReport,
                doc_date: Some(chrono::Utc::now()),
                doc_date_end: None,
                title: Some("头颅CT".into()),
                language: None,
                page_count: 1,
            })
            .unwrap();

        let (html, pass, n) = build_encrypted_share(&vault, 7).unwrap();
        assert_eq!(n, 1);
        // 自包含:内联 dicom-parser + 查看器入口都在 HTML 内。
        assert!(html.contains("dicomParser") || html.contains("dicom-parser"));
        assert!(html.contains("openDicomViewer"));
        assert!(!html.contains("/*__DICOM_PARSER__*/")); // 占位符已替换

        // 解密 payload,确认影像以交互档内嵌了原始 DICOM 字节。
        let stripped: String = pass.chars().filter(|c| !c.is_whitespace()).collect();
        let key = B64URL.decode(stripped).unwrap();
        let start = html.find("EMBEDDED_BLOB = \"").unwrap() + "EMBEDDED_BLOB = \"".len();
        let end = html[start..].find('"').unwrap() + start;
        let blob = B64.decode(&html[start..end]).unwrap();
        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        let pt = cipher
            .decrypt(
                Nonce::from_slice(&blob[..12]),
                Payload {
                    msg: &blob[12..],
                    aad: SHARE_AAD,
                },
            )
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&pt).unwrap();
        let dicom = &payload["records"][0]["dicom"];
        assert_eq!(dicom["mode"], "interactive");
        let frames = dicom["frames"].as_array().unwrap();
        assert_eq!(frames.len(), 1);
        // 帧是原始 DICOM 字节的 base64;解回应与磁盘原件一致。
        let frame0 = B64.decode(frames[0].as_str().unwrap()).unwrap();
        assert_eq!(frame0, dcm);
        assert_eq!(payload["degraded"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn round_trip_decrypt_in_rust() {
        // 加密一段已知 payload,再用同 key/nonce 在 Rust 侧解密,验证往返 + tag 布局。
        let plaintext = r#"{"hello":"世界","n":42}"#.as_bytes();
        let mut key_bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut key_bytes);
        let mut nonce_bytes = [0u8; 12];
        rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);

        let cipher = Aes256Gcm::new_from_slice(&key_bytes).unwrap();
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ct = cipher.encrypt(nonce, plaintext.as_ref()).unwrap();

        // blob = nonce || ct(含 tag)
        let mut blob = nonce_bytes.to_vec();
        blob.extend_from_slice(&ct);
        assert_eq!(blob.len(), 12 + plaintext.len() + 16); // 12 nonce + pt + 16 tag

        // 还原:切出 nonce 与密文,解密
        let iv = &blob[..12];
        let data = &blob[12..];
        let cipher2 = Aes256Gcm::new_from_slice(&key_bytes).unwrap();
        let out = cipher2.decrypt(Nonce::from_slice(iv), data).unwrap();
        assert_eq!(out, plaintext);

        // 错误密钥应解密失败
        let mut wrong = key_bytes;
        wrong[0] ^= 0xff;
        let bad = Aes256Gcm::new_from_slice(&wrong).unwrap();
        assert!(bad.decrypt(Nonce::from_slice(iv), data).is_err());
    }

    #[test]
    fn aad_round_trip_and_mismatch_fails() {
        // 用生产同款 AAD 加密 → 同 AAD 解密应还原;换 AAD 或空 AAD 必须失败。
        // 这钉住了三处解密端(Rust + 两个 JS 查看器)必须传入相同的 SHARE_AAD。
        let plaintext = br#"{"records":[]}"#;
        let mut key_bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut key_bytes);
        let mut nonce_bytes = [0u8; 12];
        rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
        let cipher = Aes256Gcm::new_from_slice(&key_bytes).unwrap();
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ct = cipher
            .encrypt(
                nonce,
                Payload {
                    msg: plaintext.as_ref(),
                    aad: SHARE_AAD,
                },
            )
            .unwrap();

        // 相同 AAD → 成功
        let out = cipher
            .decrypt(
                nonce,
                Payload {
                    msg: ct.as_ref(),
                    aad: SHARE_AAD,
                },
            )
            .unwrap();
        assert_eq!(out, plaintext);

        // 空 AAD → 失败(证明 AAD 确实参与了鉴权)
        assert!(cipher
            .decrypt(
                nonce,
                Payload {
                    msg: ct.as_ref(),
                    aad: b"",
                },
            )
            .is_err());

        // 不同 AAD → 失败
        assert!(cipher
            .decrypt(
                nonce,
                Payload {
                    msg: ct.as_ref(),
                    aad: b"medme-share-v2",
                },
            )
            .is_err());

        // 常量本身即字节串,便于人工比对 JS 的 TextEncoder().encode 结果。
        assert_eq!(SHARE_AAD, b"medme-share-v1");
    }

    #[test]
    fn passphrase_grouped_strips_back_to_key() {
        // 口令分组仅影响显示;去掉空格后应能 base64url 解回 32 字节密钥。
        let key = [7u8; 32];
        let raw = B64URL.encode(key);
        let grouped = group_passphrase(&raw);
        assert!(grouped.contains(' '));
        let stripped: String = grouped.chars().filter(|c| !c.is_whitespace()).collect();
        assert_eq!(stripped, raw);
        let decoded = B64URL.decode(stripped).unwrap();
        assert_eq!(decoded, key);
    }
}
