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

/// 唯一查看器源:自包含分享文件与托管查看器共用同一份 HTML(去重,见 #55)。
/// index.html 已内联 dicom-parser + 查看器 JS + CSP(script-src 的 sha256 已在其中固化),
/// 并自检 #share-data 切换自包含/托管模式。生成分享文件 = 取此 HTML + 注入 blob 数据节点。
const CANONICAL_VIEWER: &str = include_str!("../../../web/hosted-viewer/index.html");

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
///
/// `render_dicom_png` 是注入式 DICOM→PNG 渲染器(见 [`crate::DicomPngRenderer`]):
/// 仅在影像因体积上限降级为「锚点切片 PNG」时才会被调用。桌面必须传入子进程隔离版
/// (GHSA-24px),使本函数在主进程内绝不解码压缩像素;移动端可传
/// [`crate::render_dicom_png_in_process`]。
pub fn build_encrypted_share(
    v: &Vault,
    expires_days: u32,
    render_dicom_png: crate::DicomPngRenderer,
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

        // 内嵌 image/* 原件为 data-URI。
        let mut images: Vec<String> = Vec::new();
        if sf.mime_type.starts_with("image/") {
            let bytes = std::fs::read(v.root_join(&sf.storage_path)).map_err(|e| e.to_string())?;
            embedded_bytes += bytes.len();
            let b64 = B64.encode(&bytes);
            images.push(format!("data:{};base64,{}", sf.mime_type, b64));
        }

        // 内嵌 PDF 原件为 data-URI,让医生下载核对 OCR 原文(保真:原件是真相,见
        // docs/012)。体积不再是投递约束 —— 临时分享走「联网取密文」而非塞进聊天的
        // 文件;但仍尊重整份总上限,避免病态大文件撑爆分享。超限则跳过内嵌(仍保留
        // 识别文字),不静默。
        let mut pdf_data_uri = serde_json::Value::Null;
        if sf.mime_type == "application/pdf" {
            let bytes = std::fs::read(v.root_join(&sf.storage_path)).map_err(|e| e.to_string())?;
            if embedded_bytes + bytes.len() <= SHARE_TOTAL_CAP {
                embedded_bytes += bytes.len();
                pdf_data_uri = serde_json::Value::String(format!(
                    "data:application/pdf;base64,{}",
                    B64.encode(&bytes)
                ));
            } else {
                eprintln!("share: PDF 原件「{title}」因整份已达总上限未内嵌(仅保留识别文字)");
            }
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
                    // 经注入渲染器解码(桌面=隔离子进程,GHSA-24px),本进程不碰编解码器。
                    let png = slices
                        .first()
                        .and_then(|b| render_dicom_png(b))
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
            "pdf": pdf_data_uri,
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

    // ── 医生视图 summary(#37,slice ④)──
    // 确定性装配疾病泳道 + 趋势。`SourceDoc::index` 必须与 `records_json` 里的下标
    // 对齐(records 与 records_json 在同一循环里同序推入),证据链才能跳对原件。
    // 临床日期取文档 doc_date 的 naive 日期(无则 None)。
    let docs: Vec<parser::SourceDoc> = records
        .iter()
        .enumerate()
        .map(|(i, rec)| parser::SourceDoc {
            index: i,
            date: rec.doc.doc_date.map(|dt| dt.date_naive()),
            text: &rec.text,
        })
        .collect();
    let summary = parser::assemble_summary(&docs);

    let mut payload = serde_json::json!({
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

    // 仅当确有 problem 时挂上 summary;否则省略,查看器回退纯文档列表(老分享/空保险箱不受影响)。
    if summary["problems"]
        .as_array()
        .is_some_and(|a| !a.is_empty())
    {
        payload["summary"] = summary;
    }
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

    // 加密 blob 放进「非执行」的 JSON 数据节点(`#share-data`),查看器运行时读取。
    // 关键安全点:不再把任何 per-share 数据插进 <script> 里,两段内联脚本因此成为
    // 逐字节固定的常量,可被 CSP 的 sha256 精确收录 —— 从而移除 script-src
    // 'unsafe-inline'(GHSA-j7fx)。标准 base64 字母表不含 '<' / '"',serde 序列化
    // 后整体也不含 '<',绝无 </script> 越界之虞 —— 仍在此显式断言以防未来改动。
    let share_data = serde_json::json!({ "blob": blob_b64 }).to_string();
    if share_data.contains('<') {
        return Err("share-data 数据节点意外包含 '<'(可能 </script> 越界)".into());
    }

    // 非执行 JSON 数据节点(type=application/json,不受 CSP script-src 约束,与旧逻辑同)。
    let data_node =
        format!("<script type=\"application/json\" id=\"share-data\">{share_data}</script>");
    let marker = "<!--SHARE_DATA_SLOT-->";
    if !CANONICAL_VIEWER.contains(marker) {
        return Err("查看器模板缺少 SHARE_DATA_SLOT 注入点".into());
    }
    let html = CANONICAL_VIEWER.replacen(marker, &data_node, 1);

    Ok((html, passphrase_grouped, record_count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes_gcm::aead::Aead;

    /// 从生成 HTML 的 `#share-data` 数据节点取出 base64 blob——与浏览器查看器同源
    /// (`JSON.parse(#share-data).blob`),供各解密往返测试复用。
    fn extract_blob_b64(html: &str) -> String {
        let marker = "id=\"share-data\">";
        let start = html.find(marker).unwrap() + marker.len();
        let end = html[start..].find("</script>").unwrap() + start;
        let v: serde_json::Value = serde_json::from_str(&html[start..end]).unwrap();
        v["blob"].as_str().unwrap().to_string()
    }

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

        let (html, pass, n) =
            build_encrypted_share(&vault, 5, &crate::render_dicom_png_in_process).unwrap();
        assert_eq!(n, 1);
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("id=\"share-data\"")); // blob 移进非执行数据节点
        assert!(!html.contains("__BLOB__")); // 占位符已全部替换
        assert!(!html.contains("__EXPIRES__"));

        // 口令去空白后应能 base64url 解回 32 字节密钥。
        let stripped: String = pass.chars().filter(|c| !c.is_whitespace()).collect();
        let key = B64URL.decode(stripped).unwrap();
        assert_eq!(key.len(), 32);

        // 提取内嵌 blob → 用该密钥解密 → 应还原出合法 payload JSON(与浏览器查看器同路径)。
        let blob = B64.decode(extract_blob_b64(&html)).unwrap();
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

    /// SECURITY (GHSA-j7fx): the generated viewer's CSP must no longer allow
    /// `script-src 'unsafe-inline'` (which would let injected JS run against
    /// DECRYPTED PHI), must pin the inline scripts by exact sha256 hash, and must
    /// lock down navigation/forms so a would-be injection can't exfiltrate.
    /// Also decrypt-round-trips the produced HTML to prove behavior is preserved.
    #[test]
    fn share_viewer_csp_hardened_and_round_trips() {
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

        let (html, pass, _n) =
            build_encrypted_share(&vault, 5, &crate::render_dicom_png_in_process).unwrap();

        // 取出 CSP meta 内容。
        let marker = "Content-Security-Policy\" content=\"";
        let s = html.find(marker).unwrap() + marker.len();
        let e = html[s..].find('"').unwrap() + s;
        let csp = &html[s..e];

        // 定位 script-src 段(到下一个 ';')。
        let ss_start = csp.find("script-src ").unwrap();
        let ss_end = csp[ss_start..].find(';').unwrap() + ss_start;
        let script_src = &csp[ss_start..ss_end];

        // script-src 不得再含 'unsafe-inline';必须以 sha256 收录内联脚本。
        assert!(
            !script_src.contains("'unsafe-inline'"),
            "script-src 仍含 unsafe-inline: {script_src}"
        );
        assert!(
            script_src.contains("'sha256-"),
            "script-src 缺少 sha256 哈希: {script_src}"
        );
        // 哈希正确性现由唯一查看器源 web/hosted-viewer/index.html 固化并自测,不在此重复;
        // 这里只断言分享文件确实沿用了该 CSP(script-src 已用 sha256 收录内联脚本)。

        // 导航/表单/框架锁定,阻断潜在注入的外泄路径。
        assert!(
            csp.contains("form-action 'none'"),
            "缺 form-action 'none': {csp}"
        );
        assert!(csp.contains("base-uri 'none'"), "缺 base-uri 'none': {csp}");
        assert!(
            csp.contains("frame-ancestors 'none'"),
            "缺 frame-ancestors 'none': {csp}"
        );

        // 行为保持:内嵌 blob 用口令解密应还原 payload(与浏览器查看器同路径)。
        let stripped: String = pass.chars().filter(|c| !c.is_whitespace()).collect();
        let key = B64URL.decode(stripped).unwrap();
        let blob = B64.decode(extract_blob_b64(&html)).unwrap();
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
    }

    /// SECURITY (XSS): the share viewer must only put a data: image into `<img src>`
    /// via the strict `isDataImage` validator — never by raw string-concat of a value
    /// that merely `startsWith("data:image/")` (which a crafted `"`-carrying value
    /// could exploit to break out of the attribute and exfiltrate PHI).
    #[test]
    fn share_viewer_gates_img_sinks_through_validator() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault.import("a.txt", "text/plain", b"x").unwrap();
        let (html, _pass, _n) =
            build_encrypted_share(&vault, 5, &crate::render_dicom_png_in_process).unwrap();

        assert!(html.contains("function isDataImage"), "validator present");
        assert!(html.contains("isDataImage(img)"), "images[] sink validated");
        assert!(
            html.contains("isDataImage(dcm.png)"),
            "dcm.png sink validated"
        );
        // The old permissive check must be gone from the img sinks.
        assert!(
            !html.contains("img.startsWith(\"data:image/\")"),
            "unsafe startsWith concat removed"
        );
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

        let (html, pass, n) =
            build_encrypted_share(&vault, 7, &crate::render_dicom_png_in_process).unwrap();
        assert_eq!(n, 1);
        // 自包含:内联 dicom-parser + 查看器入口都在 HTML 内。
        assert!(html.contains("dicomParser") || html.contains("dicom-parser"));
        assert!(html.contains("openDicomViewer"));
        assert!(!html.contains("/*__DICOM_PARSER__*/")); // 占位符已替换

        // 解密 payload,确认影像以交互档内嵌了原始 DICOM 字节。
        let stripped: String = pass.chars().filter(|c| !c.is_whitespace()).collect();
        let key = B64URL.decode(stripped).unwrap();
        let blob = B64.decode(extract_blob_b64(&html)).unwrap();
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

    /// 去重后的装配契约(#55):生成的分享文件 = 唯一查看器源 index.html + 注入的
    /// `#share-data` 数据节点。断言 (a) blob 落进非执行数据节点,(b) 确实用了那份
    /// canonical 查看器(MedMe 标识 + CSP 已 sha256 收录内联脚本),(c) 注入标记已消费。
    #[test]
    fn share_html_is_canonical_viewer_with_injected_data_node() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault.import("a.txt", "text/plain", b"x").unwrap();
        let (html, _pass, _n) =
            build_encrypted_share(&vault, 5, &crate::render_dicom_png_in_process).unwrap();

        // (a) blob 在非执行 JSON 数据节点里,且可解析出 blob 字段。
        assert!(html.contains("id=\"share-data\""));
        let blob_b64 = extract_blob_b64(&html);
        assert!(!blob_b64.is_empty(), "数据节点应含非空 blob");
        assert!(B64.decode(&blob_b64).is_ok(), "blob 应为合法标准 base64");

        // (b) 沿用了唯一查看器源:MedMe 标识 + CSP 用 sha256 收录内联脚本(非本地重算)。
        assert!(
            html.contains("MedMe"),
            "应为 canonical 查看器(缺 MedMe 标识)"
        );
        assert!(
            html.contains("script-src 'sha256-"),
            "应沿用 index.html 的 CSP(script-src 已用 sha256 收录)"
        );

        // (c) 注入标记已被消费,不残留在成品里。
        assert!(
            !html.contains("<!--SHARE_DATA_SLOT-->"),
            "SHARE_DATA_SLOT 注入点应已被替换"
        );
    }

    /// 解密生成分享的 payload(与浏览器查看器同路径),供 summary 断言复用。
    fn decrypt_payload(html: &str, pass: &str) -> serde_json::Value {
        let stripped: String = pass.chars().filter(|c| !c.is_whitespace()).collect();
        let key = B64URL.decode(stripped).unwrap();
        let blob = B64.decode(extract_blob_b64(html)).unwrap();
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
        serde_json::from_slice(&pt).unwrap()
    }

    /// slice ④:含临床内容(诊断 + 化验 + 用药)的分享应带上确定性 summary,
    /// 且 problems 非空并把糖化血红蛋白/二甲双胍归到「2型糖尿病」下。
    #[test]
    fn share_includes_summary_for_clinical_records() {
        use core_model::{DocType, NewDocument, NewOcr, OcrBackendKind};
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let imp = vault.import("门诊.txt", "text/plain", b"data").unwrap();
        let doc = vault
            .add_document(NewDocument {
                source_file_id: imp.source_file.id,
                doc_type: DocType::ClinicalNote,
                doc_date: Some(chrono::Utc::now()),
                doc_date_end: None,
                title: Some("内分泌门诊".into()),
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
                text: "诊断:2型糖尿病\n糖化血红蛋白 7.9 % 4-6.5\n二甲双胍 0.5g bid".into(),
                confidence: None,
            })
            .unwrap();

        let (html, pass, _n) =
            build_encrypted_share(&vault, 5, &crate::render_dicom_png_in_process).unwrap();
        let payload = decrypt_payload(&html, &pass);
        let problems = payload["summary"]["problems"]
            .as_array()
            .expect("summary.problems 应存在");
        assert!(!problems.is_empty());
        assert!(problems.iter().any(|p| p["term"] == "2型糖尿病"));
    }

    /// slice ④:纯非临床记录(无诊断/化验/用药)不产出任何 problem,故 payload 里
    /// 不应有 summary 键——查看器回退纯文档列表,老分享/空保险箱不受影响。
    #[test]
    fn share_omits_summary_for_non_clinical_records() {
        use core_model::{DocType, NewDocument, NewOcr, OcrBackendKind};
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let imp = vault.import("须知.txt", "text/plain", b"data").unwrap();
        let doc = vault
            .add_document(NewDocument {
                source_file_id: imp.source_file.id,
                doc_type: DocType::Other,
                doc_date: Some(chrono::Utc::now()),
                doc_date_end: None,
                title: Some("复诊须知".into()),
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
                text: "复诊须知:请按时到内分泌科随访,携带既往病历。".into(),
                confidence: None,
            })
            .unwrap();

        let (html, pass, _n) =
            build_encrypted_share(&vault, 5, &crate::render_dicom_png_in_process).unwrap();
        let payload = decrypt_payload(&html, &pass);
        assert!(
            payload.get("summary").is_none(),
            "非临床分享不应带 summary 键"
        );
    }
}
