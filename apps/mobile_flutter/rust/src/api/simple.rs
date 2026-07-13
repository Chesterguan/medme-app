#[flutter_rust_bridge::frb(sync)] // Synchronous mode for simplicity of the demo
pub fn greet(name: String) -> String {
    format!("Hello, {name}!")
}

/// P1 冒烟:证明现有 Rust 数据核(core-model)能经 FRB 从 Flutter 调通。
/// 在给定目录打开一个保险箱(不存在则新建),返回时间线记录数。
/// 后续 P2 会把 load_archive / ingest / share / export 等全量 API 都接进来。
pub fn vault_smoke(dir: String) -> anyhow::Result<String> {
    let vault = core_model::Vault::open(std::path::Path::new(&dir))?;
    let n = vault
        .timeline()
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .len();
    Ok(format!("Rust 核已连通 · 保险箱在 {dir} · {n} 条记录"))
}

#[flutter_rust_bridge::frb(init)]
pub fn init_app() {
    // Default utilities - feel free to customize
    flutter_rust_bridge::setup_default_user_utils();
}
