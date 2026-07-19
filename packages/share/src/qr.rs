//! 二维码分享(阶段一):把「当下病情」压进一张二维码,面对面给医生扫。
//!
//! 场景:门诊里医生用自己的手机扫患者屏幕上的码 —— 不经微信、不过医院网闸、
//! 不需要医生装任何东西。要看原件或阅片,患者手机当场翻。
//!
//! **硬约束是二维码容量**:二进制模式上限 2953 字节(version 40 + 最低纠错),
//! 还要留出 URL 前缀。而 summary 体积随病历份数增长 —— 实测约 80–100 份就会撑破。
//! 因此这里**按构造裁剪**(见 [`QrLimits`]),而不是「希望它装得下」:只带在治的病、
//! 每个指标最近几个点、在用的药。裁剪后体积与病历总量脱钩。
//!
//! 载荷 = gzip(JSON) → AES-256-GCM → base64url,放进 URL fragment。`#` 之后按 HTTP
//! 规范不上行,故密钥与密文都只存在于收件人浏览器本地(查看器侧见 hosted-viewer)。

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;
use serde_json::{json, Value};

/// 二维码二进制模式容量上限(version 40,纠错等级 L)。
pub const QR_BINARY_CAPACITY: usize = 2953;

/// AEAD 绑定串:与整份分享(`medme-share-v1`)区分开,避免两种载荷被互相当成对方解。
const QR_AAD: &[u8] = b"medme-qr-v1";

/// fragment 版本前缀。查看器据此区分二维码载荷与口令。
pub const FRAGMENT_PREFIX: &str = "q1.";

/// 裁剪参数。默认值是**工程占位**,临床上该带多少由医生侧反馈决定 —— 改这里即可,
/// 不需要动管线。
#[derive(Debug, Clone, Copy)]
pub struct QrLimits {
    /// 最多带几个疾病(按「需关注 > 在治 > 其它」排序后取前 N)。
    pub max_problems: usize,
    /// 每个疾病最多带几条化验序列。
    pub max_labs_per_problem: usize,
    /// 每条化验序列最多带最近几个点。
    pub max_points_per_lab: usize,
    /// 每个疾病最多带几条用药。
    pub max_meds_per_problem: usize,
    /// 只带在用的药(停用的略去)。
    pub active_meds_only: bool,
    /// 最多带几条「近期变化」。
    pub max_notable_changes: usize,
}

impl Default for QrLimits {
    fn default() -> Self {
        Self {
            max_problems: 6,
            max_labs_per_problem: 2,
            max_points_per_lab: 4,
            max_meds_per_problem: 3,
            active_meds_only: true,
            max_notable_changes: 3,
        }
    }
}

/// 已裁剪的载荷 + 其分享链接片段。
pub struct QrShare {
    /// `#` 之后的内容:`q1.<base64url(nonce‖密文)>.<base64url(密钥)>`。
    /// `q1.` 前缀让查看器一眼区分「二维码载荷」与「口令」两种 fragment。
    pub fragment: String,
    /// 加密后的字节数(不含 base64 膨胀),用于判断是否还塞得进二维码。
    pub cipher_len: usize,
    /// 裁剪后实际带上的疾病数,便于 UI 告诉患者「本码含 N 个在治问题」。
    pub problem_count: usize,
}

impl QrShare {
    /// 连同 URL 前缀在内的总字节数是否仍在二维码容量内。
    pub fn fits_qr(&self, url_prefix: &str) -> bool {
        url_prefix.len() + 1 + self.fragment.len() <= QR_BINARY_CAPACITY
    }
}

/// 按 [`QrLimits`] 裁剪一份完整 summary。输入是 `parser::assemble_summary` 的产物,
/// 输出结构与之同形 —— 查看器因此不需要第二套渲染逻辑。
pub fn trim_summary(summary: &Value, lim: QrLimits) -> Value {
    let take_str = |v: &Value, n: usize| -> Vec<Value> {
        v.as_array()
            .map(|a| a.iter().take(n).cloned().collect())
            .unwrap_or_default()
    };

    let mut problems: Vec<Value> = summary["problems"].as_array().cloned().unwrap_or_default();
    // 需关注的排前面,其次仍在治的,已愈的最后 —— 容量不够时先丢掉已经好了的。
    problems.sort_by_key(|p| {
        let warn = p["warn"].as_bool().unwrap_or(false);
        let healed = p["end"].is_string();
        match (warn, healed) {
            (true, false) => 0,
            (false, false) => 1,
            _ => 2,
        }
    });
    problems.truncate(lim.max_problems);

    let trimmed: Vec<Value> = problems
        .iter()
        .map(|p| {
            let labs: Vec<Value> = p["labs"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .take(lim.max_labs_per_problem)
                        .map(|l| {
                            let mut l = l.clone();
                            if let Some(pts) = l["pts"].as_array() {
                                // 只保留最近若干个点(pts 按时间升序,取尾部)。
                                let start = pts.len().saturating_sub(lim.max_points_per_lab);
                                l["pts"] = json!(pts[start..].to_vec());
                            }
                            l
                        })
                        .collect()
                })
                .unwrap_or_default();

            let meds: Vec<Value> = p["meds"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter(|m| !lim.active_meds_only || m["on"].as_bool().unwrap_or(false))
                        .take(lim.max_meds_per_problem)
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();

            let mut p = p.clone();
            p["labs"] = json!(labs);
            p["meds"] = json!(meds);
            // evidence 指向完整分享里的记录下标,二维码里没有记录数组,留着会指向空。
            p.as_object_mut().map(|o| o.remove("evidence"));
            p
        })
        .collect();

    let mut out = json!({
        "problems": trimmed,
        "allergies": summary["allergies"].clone(),
        "notable_changes": take_str(&summary["notable_changes"], lim.max_notable_changes),
    });
    // 影像与病理只带结论文本(原件在患者手机上),没有就不带这个键。
    for key in ["imaging", "pathology"] {
        let v = take_str(&summary[key], 2);
        if !v.is_empty() {
            out[key] = json!(v);
        }
    }
    out
}

/// 把裁剪后的 summary 打成二维码载荷:gzip → AES-256-GCM → base64url。
///
/// `patient` 是给医生看的抬头(姓名/性别/年龄),`key_bytes`/`nonce_bytes` 由调用方
/// 提供随机源,便于测试注入固定值。
pub fn build_qr_fragment(
    trimmed_summary: &Value,
    patient: &Value,
    generated_iso: &str,
    key_bytes: [u8; 32],
    nonce_bytes: [u8; 12],
) -> Result<QrShare, String> {
    let problem_count = trimmed_summary["problems"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    let payload = json!({
        "v": 1,
        "kind": "qr-summary",
        "generated": generated_iso,
        "patient": patient,
        "summary": trimmed_summary,
    });

    let raw = serde_json::to_vec(&payload).map_err(|e| format!("serialize qr payload: {e}"))?;
    let gz = {
        use std::io::Write;
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        e.write_all(&raw).map_err(|e| format!("gzip: {e}"))?;
        e.finish().map_err(|e| format!("gzip finish: {e}"))?
    };

    let cipher = Aes256Gcm::new_from_slice(&key_bytes).map_err(|e| format!("init cipher: {e}"))?;
    let nonce: &Nonce<_> = (&nonce_bytes).into();
    let ct = cipher
        .encrypt(
            nonce,
            Payload {
                msg: &gz,
                aad: QR_AAD,
            },
        )
        .map_err(|e| format!("encrypt: {e}"))?;

    let mut blob = Vec::with_capacity(12 + ct.len());
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ct);

    Ok(QrShare {
        fragment: format!(
            "{}{}.{}",
            FRAGMENT_PREFIX,
            B64URL.encode(&blob),
            B64URL.encode(key_bytes)
        ),
        cipher_len: blob.len(),
        problem_count,
    })
}

/// 从保险箱直接产出二维码分享:装配 summary → 按 [`QrLimits`] 裁剪 → 加密成
/// fragment。随机源用 OS 熵(失败按错误上报,不 panic)。
///
/// 返回 `(完整 URL, QrShare)`;URL 形如 `<base_url>#q1.<密文>.<密钥>`,直接编成
/// 二维码即可。密钥在 `#` 之后 —— 按 HTTP 规范不会发给服务器。
pub fn build_qr_share(
    v: &core_model::Vault,
    base_url: &str,
    lim: QrLimits,
) -> Result<(String, QrShare), String> {
    let records = crate::export::gather_records(v)?;
    let profile = pipeline::patient_profile(v).map_err(|e| e.to_string())?;
    let docs: Vec<parser::SourceDoc<'_>> = records
        .iter()
        .enumerate()
        .map(|(i, rec)| parser::SourceDoc {
            index: i,
            date: rec.doc.doc_date.map(|dt| dt.date_naive()),
            text: &rec.text,
            doc_type: Some(rec.doc.doc_type.as_str().to_lowercase()),
            title: rec.doc.title.clone(),
        })
        .collect();
    let summary = parser::assemble_summary(&docs);
    let trimmed = trim_summary(&summary, lim);

    let mut key = [0u8; 32];
    getrandom::fill(&mut key).map_err(|e| format!("OS RNG unavailable: {e}"))?;
    let mut nonce = [0u8; 12];
    getrandom::fill(&mut nonce).map_err(|e| format!("OS RNG unavailable: {e}"))?;

    let patient = serde_json::json!({
        "name": profile.name,
        "gender": profile.gender,
        "age": profile.age,
    });
    let qr = build_qr_fragment(
        &trimmed,
        &patient,
        &chrono::Utc::now().to_rfc3339(),
        key,
        nonce,
    )?;
    let url = format!("{}#{}", base_url.trim_end_matches('#'), qr.fragment);
    Ok((url, qr))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_summary(n_problems: usize, pts_per_lab: usize) -> Value {
        let problems: Vec<Value> = (0..n_problems)
            .map(|i| {
                let pts: Vec<Value> = (0..pts_per_lab)
                    .map(|j| json!([format!("202{}-06", j % 7), 7.2 + j as f64 * 0.1]))
                    .collect();
                json!({
                    "term": format!("慢性病{i}"),
                    "onset": "2021-05",
                    "status": "控制中",
                    "warn": i % 3 == 0,
                    "evidence": [0, 1, 2],
                    "labs": [
                        {"name": format!("指标{i}甲"), "short": "A", "unit": "mmol/L",
                         "refHigh": 6.5, "pts": pts.clone(), "evidence": [0]},
                        {"name": format!("指标{i}乙"), "short": "B", "unit": "g/L",
                         "refLow": 130, "pts": pts.clone(), "evidence": [0]},
                        {"name": format!("指标{i}丙"), "short": "C", "unit": "%",
                         "refHigh": 3.4, "pts": pts, "evidence": [0]}
                    ],
                    "meds": [
                        {"name": format!("药{i}甲"), "dose": "0.5g bid", "on": true, "span": "自 2021", "evidence": [1]},
                        {"name": format!("药{i}乙"), "dose": "5mg qd", "on": false, "span": "2020 → 2024 停", "evidence": [1]}
                    ]
                })
            })
            .collect();
        json!({
            "problems": problems,
            "allergies": [{"substance": "青霉素", "reaction": "皮疹"}],
            "notable_changes": ["糖化血红蛋白 7.9 → 7.2%", "新增高脂血症", "阿司匹林停用", "第四条", "第五条"],
        })
    }

    fn build(summary: &Value) -> QrShare {
        build_qr_fragment(
            summary,
            &json!({"name": "张三", "gender": "男", "age": "58"}),
            "2026-07-18T00:00:00Z",
            [7u8; 32],
            [3u8; 12],
        )
        .unwrap()
    }

    #[test]
    fn trims_to_the_configured_bounds() {
        let lim = QrLimits::default();
        let t = trim_summary(&sample_summary(20, 12), lim);
        let problems = t["problems"].as_array().unwrap();
        assert_eq!(problems.len(), lim.max_problems, "疾病数应被裁到上限");
        for p in problems {
            assert!(p["labs"].as_array().unwrap().len() <= lim.max_labs_per_problem);
            for l in p["labs"].as_array().unwrap() {
                assert!(l["pts"].as_array().unwrap().len() <= lim.max_points_per_lab);
            }
            // 只留在用的药
            for m in p["meds"].as_array().unwrap() {
                assert!(m["on"].as_bool().unwrap_or(false), "停用的药不该进二维码");
            }
            assert!(
                p.get("evidence").is_none(),
                "二维码里没有记录数组,evidence 应剥掉"
            );
        }
        assert_eq!(
            t["notable_changes"].as_array().unwrap().len(),
            lim.max_notable_changes
        );
    }

    #[test]
    fn warn_problems_survive_truncation() {
        // warn 的疾病应排在前面,被截断时先保住它们。
        let t = trim_summary(&sample_summary(20, 4), QrLimits::default());
        let first = &t["problems"].as_array().unwrap()[0];
        assert_eq!(first["warn"], json!(true));
    }

    /// 这条是整个方案的地基:**裁剪后体积必须与病历总量脱钩**,否则慢病随访患者
    /// 一定会撑破二维码(实测未裁剪时约 80–100 份就超)。
    #[test]
    fn payload_fits_a_qr_code_regardless_of_history_size() {
        const PREFIX: &str = "https://chesterguan.github.io/medme/v";
        let lim = QrLimits::default();
        let mut sizes = Vec::new();
        for n in [5usize, 20, 60, 200, 500] {
            let trimmed = trim_summary(&sample_summary(n, 40), lim);
            let qr = build(&trimmed);
            assert!(
                qr.fits_qr(PREFIX),
                "{n} 个疾病时载荷 {} 字节,超出二维码容量",
                qr.fragment.len()
            );
            sizes.push(qr.fragment.len());
        }
        // 体积应基本恒定:最大与最小相差不超过 5%。
        let (min, max) = (sizes.iter().min().unwrap(), sizes.iter().max().unwrap());
        assert!(
            (*max as f64) < (*min as f64) * 1.05,
            "裁剪后体积应与病历总量脱钩,实测 {sizes:?}"
        );
    }

    #[test]
    fn round_trips_through_decrypt_and_gunzip() {
        use std::io::Read;
        let trimmed = trim_summary(&sample_summary(8, 10), QrLimits::default());
        let qr = build(&trimmed);

        let (blob_b64, key_b64) = qr
            .fragment
            .strip_prefix(FRAGMENT_PREFIX)
            .expect("应带版本前缀")
            .split_once('.')
            .expect("fragment 应为 q1.数据.密钥");
        let blob = B64URL.decode(blob_b64).unwrap();
        let key = B64URL.decode(key_b64).unwrap();

        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        let gz = cipher
            .decrypt(
                (&blob[..12]).try_into().unwrap(),
                Payload {
                    msg: &blob[12..],
                    aad: QR_AAD,
                },
            )
            .expect("应能解密");
        let mut out = String::new();
        flate2::read::GzDecoder::new(&gz[..])
            .read_to_string(&mut out)
            .unwrap();
        let back: Value = serde_json::from_str(&out).unwrap();

        assert_eq!(back["kind"], "qr-summary");
        assert_eq!(back["patient"]["name"], "张三");
        assert_eq!(back["summary"]["problems"], trimmed["problems"]);
    }

    #[test]
    fn wrong_aad_fails_to_decrypt() {
        // 整份分享用的是 medme-share-v1;两种载荷不该互相解开。
        let trimmed = trim_summary(&sample_summary(3, 4), QrLimits::default());
        let qr = build(&trimmed);
        let body = qr.fragment.strip_prefix(FRAGMENT_PREFIX).unwrap();
        let (blob_b64, key_b64) = body.split_once('.').unwrap();
        let blob = B64URL.decode(blob_b64).unwrap();
        let key = B64URL.decode(key_b64).unwrap();
        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        assert!(cipher
            .decrypt(
                (&blob[..12]).try_into().unwrap(),
                Payload {
                    msg: &blob[12..],
                    aad: b"medme-share-v1"
                }
            )
            .is_err());
    }

    #[test]
    #[ignore = "度量用,cargo test -- --ignored --nocapture 查看"]
    fn print_sizes() {
        const PREFIX: &str = "https://chesterguan.github.io/medme/v";
        println!(
            "二维码容量上限 {QR_BINARY_CAPACITY} 字节,URL 前缀占 {}",
            PREFIX.len()
        );
        for n in [5usize, 20, 60, 200, 500] {
            let qr = build(&trim_summary(&sample_summary(n, 40), QrLimits::default()));
            println!(
                "  {n:>3} 个疾病 → 裁到 {} 个 · 密文 {} 字节 · fragment {} 字符 · 进码 {}",
                qr.problem_count,
                qr.cipher_len,
                qr.fragment.len(),
                if qr.fits_qr(PREFIX) { "是" } else { "否" }
            );
        }
    }
}
