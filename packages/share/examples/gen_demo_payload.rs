//! 生成示例查看器的 payload:把张建国的真实示例数据集导入保险箱,走**生产的**
//! `build_encrypted_share` 产出分享,再解密取出明文 payload 写成 JSON。
//!
//! 这样示例页的数据结构与真实分享逐字段一致 —— 因为它就是同一条代码路径产出的,
//! 只是数据换成了公开的演示数据集。
//!
//! cargo run --release -p medme-share --example gen_demo_payload -- <输出路径>
use base64::Engine;
use core_model::Vault;

fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect(&p, out);
            } else if p.is_file() {
                out.push(p);
            }
        }
    }
}

fn main() {
    let out_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "web/hosted-viewer/demo-payload.json".into());
    let mut files = Vec::new();
    collect(
        std::path::Path::new("apps/desktop/src-tauri/demo-data"),
        &mut files,
    );
    // 12 层头颅 CT 序列:生产会把同一 study 的多张 .dcm 归成一叠可滚轮翻层
    collect(
        std::path::Path::new("examples/demo-dataset/imaging/头颅CT序列"),
        &mut files,
    );
    files.sort();
    println!("示例数据集 {} 个文件", files.len());

    let tmp = tempfile::tempdir().unwrap();
    let vault = Vault::open(tmp.path()).unwrap();
    let mut ok = 0usize;
    for p in &files {
        match pipeline::ingest(&vault, p) {
            Ok(_) => ok += 1,
            Err(e) => eprintln!("  跳过 {}: {e}", p.display()),
        }
    }
    vault.rebuild_encounters().unwrap();
    println!("导入成功 {ok}");

    let (html, pass, n) = medme_share::share::build_encrypted_share(
        &vault,
        5,
        &medme_share::render_dicom_png_in_process,
    )
    .unwrap();
    println!("分享含 {n} 份记录,HTML {} MB", html.len() / 1024 / 1024);

    // 解密取明文 payload(示例页不需要加密:它本来就是公开演示数据)
    let s = html.find("id=\"share-data\"").unwrap();
    let o = html[s..].find('>').unwrap() + s + 1;
    let c = html[o..].find("</script>").unwrap() + o;
    let node: serde_json::Value = serde_json::from_str(html[o..c].trim()).unwrap();
    let blob = base64::engine::general_purpose::STANDARD
        .decode(node["blob"].as_str().unwrap())
        .unwrap();
    let key = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(
            pass.chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>(),
        )
        .unwrap();
    let plain = {
        use aes_gcm::aead::{Aead, KeyInit, Payload};
        let ci = aes_gcm::Aes256Gcm::new_from_slice(&key).unwrap();
        ci.decrypt(
            (&blob[..12]).try_into().unwrap(),
            Payload {
                msg: &blob[12..],
                aad: b"medme-share-v1",
            },
        )
        .unwrap()
    };
    std::fs::write(&out_path, &plain).unwrap();
    println!("写出 {out_path}:{} MB", plain.len() / 1024 / 1024);
}
