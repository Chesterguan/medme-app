use clap::{Parser, Subcommand};
use core_model::{NewDocument, NewOcr, OcrBackendKind, Vault};
use std::path::PathBuf;

#[derive(Parser)]
struct Cli {
    #[arg(long)]
    vault: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    Import { files: Vec<PathBuf> },
    Search { query: String },
    Timeline,
}

fn mime_for(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        _ => "application/octet-stream",
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let vault = Vault::open(&cli.vault)?;

    match cli.cmd {
        Cmd::Import { files } => {
            for f in files {
                let bytes = std::fs::read(&f)?;
                let name = f
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                let imp = vault.import(&name, mime_for(&f), &bytes)?;
                if imp.deduped {
                    println!("dedup  {name} (already stored, id={})", imp.source_file.id);
                    continue;
                }
                // 文本层抽取;失败(如扫描件图片)不致命,留给后续 OCR 计划
                match parser::extract(&f) {
                    Ok(e) => {
                        let doc = vault.add_document(NewDocument {
                            source_file_id: imp.source_file.id,
                            doc_type: e.doc_type,
                            doc_date: e.doc_date,
                            title: Some(name.clone()),
                            language: e.language,
                            page_count: e.page_count,
                        })?;
                        vault.add_ocr(NewOcr {
                            document_id: doc.id,
                            page_no: 1,
                            backend: OcrBackendKind::Native,
                            model_version: "text-layer".into(),
                            text: e.text,
                            confidence: None,
                        })?;
                        println!(
                            "import {name} (id={}, type={})",
                            imp.source_file.id,
                            doc.doc_type.as_str()
                        );
                    }
                    Err(err) => {
                        println!("import {name} (stored, no text layer: {err})");
                    }
                }
            }
        }
        Cmd::Search { query } => {
            let hits = vault.search(&query, 20)?;
            if hits.is_empty() {
                println!("no matches");
            }
            for h in hits {
                println!("#{}  {}", h.document_id, h.snippet);
            }
        }
        Cmd::Timeline => {
            for e in vault.timeline()? {
                let date = e
                    .doc_date
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| "无日期".into());
                println!(
                    "{date}  [{}]  {}",
                    e.doc_type.as_str(),
                    e.title.unwrap_or_default()
                );
            }
        }
    }
    Ok(())
}
