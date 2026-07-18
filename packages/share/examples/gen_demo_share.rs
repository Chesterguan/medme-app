//! 生成示例查看器页面:把张建国的真实示例数据集导入保险箱,走**生产的**
//! `build_encrypted_share` 产出一份**真实加密分享**,再注入一条「示例数据」横幅。
//!
//! 示例页与真实分享不是「机制相同」,而是**同一条代码路径的产物** —— 同一份查看器、
//! 同一套 AES-256-GCM、同一个口令流程,只有数据换成公开演示数据集。口令随 URL
//! fragment 送达(`demo/#<口令>`),`#` 之后不上行,与二维码/链接分享一致。
//!
//! cargo run --release -p medme-share --example gen_demo_share -- <输出 html> [口令输出文件]

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
        .unwrap_or_else(|| "demo/index.html".into());
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
        3650, // 示例页不该显示「已过期」提示
        &medme_share::render_dicom_png_in_process,
    )
    .unwrap();

    // 示例数据必须显著标注 —— 否则这一页读起来就是一份真实病历。横幅是**静态 HTML**,
    // 不碰查看器脚本(改脚本会使 CSP 锁死的 sha256 失配,整段脚本被拒绝执行)。
    let banner = "<div role=\"note\" style=\"position:sticky;top:0;z-index:50;background:#fef3c7;\
border-bottom:1px solid #fcd34d;color:#92400e;font-size:13px;font-weight:600;padding:9px 16px;\
text-align:center;line-height:1.5\">示例演示 · 患者「张建国」及其全部化验、用药、影像、病理均为公开演示数据,\
非真实病历。本页就是一份真实的 MedMe 加密分享,功能与患者发给医生的完全一致。</div>";
    let html = html.replacen("<body>", &format!("<body>{banner}"), 1);

    std::fs::write(&out_path, &html).unwrap();
    println!("写出 {out_path}:{} MB,{n} 份记录", html.len() / 1024 / 1024);

    if let Some(pass_path) = std::env::args().nth(2) {
        std::fs::write(&pass_path, &pass).unwrap();
        println!("口令写入 {pass_path}");
    } else {
        println!("口令:{pass}");
    }
}
