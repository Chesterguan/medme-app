//! 导出 v1:把整条时间线渲染成一份自包含 HTML —— 可在任意浏览器打开、原生
//! 渲染中文、并通过浏览器自带的“打印 / 另存为 PDF”交给医生。
//!
//! 不用 Rust 端 PDF 库,是因为 CJK 字体在那些库里需要手动嵌入、体积大且脆弱;
//! 浏览器/系统 webview 自带中文字体,HTML+CSS 打印天然支持分页与 CJK,
//! 同时这份 HTML 也是未来分享查看器(share-viewer)的雏形。

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use core_model::{DocType, SourceFile, Vault};

/// 转义 HTML 特殊字符,避免标题/OCR 文本里的 `<`、`&` 等破坏页面结构。
fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// 一条按病程正序整理好的记录:文档 + 原件 + OCR 文本。时间线导出与加密分享
/// (`crate::share`)共用此遍历逻辑,避免重复走 vault。
pub(crate) struct GatheredRecord {
    pub doc: core_model::Document,
    pub source_file: SourceFile,
    pub text: String,
}

/// 按病程正序(旧→新,无日期最后)遍历 vault,取出每条文档的原件与 OCR 文本。
pub(crate) fn gather_records(vault: &Vault) -> Result<Vec<GatheredRecord>, String> {
    // Vault::timeline() 按日期倒序(无日期最后);正序更利于阅读,反转后把无日期挪到末尾。
    let mut entries = vault.timeline().map_err(|e| e.to_string())?;
    entries.reverse();
    let (mut dated, undated): (Vec<_>, Vec<_>) =
        entries.into_iter().partition(|e| e.doc_date.is_some());
    dated.extend(undated);

    let mut out = Vec::new();
    for entry in &dated {
        let Some(doc) = vault
            .document_by_id(entry.document_id)
            .map_err(|e| e.to_string())?
        else {
            continue;
        };
        let Some(sf) = vault
            .source_file_by_id(doc.source_file_id)
            .map_err(|e| e.to_string())?
        else {
            continue;
        };
        let text = vault.ocr_text(doc.id).map_err(|e| e.to_string())?;
        out.push(GatheredRecord {
            doc,
            source_file: sf,
            text,
        });
    }
    Ok(out)
}

/// 与前端 `docmeta.ts` 的 `TYPE_LABEL` 保持一致的中文类型徽标。
pub(crate) fn doc_type_label(t: &DocType) -> &'static str {
    match t {
        DocType::LabReport => "化验",
        DocType::ImagingReport => "检查",
        DocType::DischargeSummary => "出院",
        DocType::Prescription => "处方",
        DocType::ClinicalNote => "病历",
        DocType::Pathology => "病理",
        DocType::Surgery => "手术",
        DocType::Other => "其他",
        DocType::Unknown => "未分类",
    }
}

fn fmt_date(d: Option<chrono::DateTime<chrono::Utc>>) -> Option<String> {
    d.map(|x| x.format("%Y-%m-%d").to_string())
}

/// 为图片/DICOM 原件生成内嵌预览块;PDF 及其他类型不内嵌(仅保留文字与文件名),
/// 避免额外的解码/渲染成本。
///
/// 影像(DICOM)导出的是“轻档 · 关键切片”(014 §4):把该检查的**锚点切片**经注入式
/// `render_dicom_png`(见 [`crate::DicomPngRenderer`])渲成 PNG 内嵌,配一句“完整序列见
/// 分享”的说明,像胶片一样能打印。桌面把该渲染器指向隔离子进程(GHSA-24px),故本函数
/// 在主进程内绝不解码压缩像素。渲染务必稳健:遇到不支持的压缩(渲染器返回 `None`)或读盘
/// 失败时**降级为一行说明**,绝不因单条影像中断整份导出。
fn render_preview(
    vault: &Vault,
    sf: &SourceFile,
    render_dicom_png: crate::DicomPngRenderer,
) -> Result<Option<String>, String> {
    if sf.mime_type.starts_with("image/") {
        let bytes = std::fs::read(vault.root_join(&sf.storage_path)).map_err(|e| e.to_string())?;
        let b64 = B64.encode(&bytes);
        // SECURITY: escape the mime_type before it goes into the src attribute (it is
        // shape-validated at ingest, but escape here too — defense-in-depth so a value
        // carrying a `"` can never break out of the attribute and inject markup).
        return Ok(Some(format!(
            "<img class=\"preview\" src=\"data:{};base64,{}\" alt=\"原件预览\">\n",
            escape_html(&sf.mime_type),
            b64
        )));
    }
    if sf.mime_type == "application/dicom" {
        // 读盘或渲染失败都降级为说明,不中断导出。渲染经注入器(桌面=隔离子进程,
        // GHSA-24px),本进程不碰编解码器。
        let png = std::fs::read(vault.root_join(&sf.storage_path))
            .ok()
            .and_then(|bytes| render_dicom_png(&bytes));
        return Ok(Some(match png {
            Some(png) => format!(
                "<figure class=\"imaging\"><img class=\"preview\" src=\"data:image/png;base64,{}\" alt=\"影像关键切片\"><figcaption class=\"caption\">影像:关键切片(完整序列见分享)</figcaption></figure>\n",
                B64.encode(&png)
            ),
            None => "<div class=\"note\">(影像原件为不支持的压缩格式,未能生成关键切片预览;完整序列请见加密分享)</div>\n".to_string(),
        }));
    }
    Ok(None)
}

fn format_patient_line(p: &pipeline::PatientProfile) -> String {
    let mut parts = Vec::new();
    if let Some(n) = &p.name {
        parts.push(n.clone());
    }
    if let Some(g) = &p.gender {
        parts.push(g.clone());
    }
    if let Some(b) = &p.birth_date {
        parts.push(format!("生于 {b}"));
    }
    if let Some(a) = &p.age {
        parts.push(format!("{a}岁"));
    }
    if parts.is_empty() {
        "（未从原件中识别到患者基本信息）".to_string()
    } else {
        parts.join(" · ")
    }
}

/// 构建整条时间线的自包含导出 HTML。返回 `(html, 记录数)`。
///
/// `render_dicom_png` 是注入式 DICOM→PNG 渲染器(见 [`crate::DicomPngRenderer`]):
/// 桌面必须传入子进程隔离版(GHSA-24px),使本函数在主进程内绝不解码压缩像素。
pub fn build_timeline_html(
    vault: &Vault,
    render_dicom_png: crate::DicomPngRenderer,
) -> Result<(String, i64), String> {
    build_timeline_html_ranged(vault, render_dicom_png, None, None)
}

/// 与 [`build_timeline_html`] 相同,但只导出 `doc_date` 落在 `[from, to]`(含端点)
/// 区间内的记录。`from`/`to` 任一为 `None` 表示该侧不限;两者都为 `None` 即全量
/// (等价于 [`build_timeline_html`])。**无 `doc_date` 的记录仅在完全不筛选时纳入**
/// ——一旦指定了任一端点,无日期记录无法归入区间,予以排除(行为可预期)。
pub fn build_timeline_html_ranged(
    vault: &Vault,
    render_dicom_png: crate::DicomPngRenderer,
    from: Option<chrono::DateTime<chrono::Utc>>,
    to: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<(String, i64), String> {
    let records: Vec<GatheredRecord> = gather_records(vault)?
        .into_iter()
        .filter(|rec| match rec.doc.doc_date {
            Some(d) => from.is_none_or(|f| d >= f) && to.is_none_or(|t| d <= t),
            None => from.is_none() && to.is_none(),
        })
        .collect();
    let profile = pipeline::patient_profile(vault).map_err(|e| e.to_string())?;

    let mut body = String::new();
    let mut record_count: i64 = 0;

    for rec in &records {
        let doc = &rec.doc;
        let sf = &rec.source_file;
        let text = &rec.text;

        let title = doc
            .title
            .clone()
            .unwrap_or_else(|| sf.original_name.clone());
        let date_str = match (fmt_date(doc.doc_date), fmt_date(doc.doc_date_end)) {
            (Some(a), Some(b)) if a != b => format!("{a} → {b}"),
            (Some(a), _) => a,
            (None, _) => "无日期".to_string(),
        };

        let preview = render_preview(vault, sf, render_dicom_png)?;

        body.push_str("<section class=\"record\">\n");
        body.push_str(&format!(
            "<div class=\"record-head\"><span class=\"badge\">{}</span><h2>{}</h2><span class=\"date\">{}</span></div>\n",
            escape_html(doc_type_label(&doc.doc_type)),
            escape_html(&title),
            escape_html(&date_str),
        ));
        body.push_str(&format!(
            "<div class=\"meta\">原始文件:{}({})</div>\n",
            escape_html(&sf.original_name),
            escape_html(&sf.mime_type),
        ));
        if let Some(img_tag) = preview {
            body.push_str(&img_tag);
        }
        if !text.trim().is_empty() {
            body.push_str(&format!(
                "<pre class=\"ocr-text\">{}</pre>\n",
                escape_html(text)
            ));
        } else if sf.mime_type == "application/pdf" {
            body.push_str("<div class=\"note\">(PDF 原件未内嵌预览,请参见原始文件)</div>\n");
        }
        body.push_str("</section>\n");
        record_count += 1;
    }

    let patient_line = format_patient_line(&profile);
    let generated_at = chrono::Utc::now().format("%Y-%m-%d %H:%M");
    let html = format!(
        r#"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data:; style-src 'unsafe-inline'; script-src 'none'; form-action 'none'; base-uri 'none'">
<title>MedMe 医疗时间线导出</title>
<style>{CSS}</style>
</head>
<body>
<header class="doc-header">
  <h1>MedMe 医我 · 医疗时间线导出</h1>
  <div class="patient">{}</div>
  <div class="generated">生成时间:{generated_at} · 共 {record_count} 份记录</div>
</header>
<main>
{body}
</main>
<footer class="statement">本导出由 MedMe 生成,不构成医疗建议;数据以原件为准。</footer>
</body>
</html>
"#,
        escape_html(&patient_line),
    );

    Ok((html, record_count))
}

const CSS: &str = r#"
  * { box-sizing: border-box; }
  body { font-family: -apple-system, "PingFang SC", "Microsoft YaHei", "Noto Sans CJK SC", "Segoe UI", sans-serif; color: #1e293b; margin: 0; padding: 24px; max-width: 900px; margin-inline: auto; background: #f8fafc; }
  .doc-header { border-bottom: 2px solid #2563eb; padding-bottom: 12px; margin-bottom: 20px; }
  .doc-header h1 { font-size: 22px; color: #1d4ed8; margin: 0 0 6px; }
  .patient { font-size: 14px; color: #334155; }
  .generated { font-size: 12px; color: #94a3b8; margin-top: 4px; }
  .record { background: #fff; border: 1px solid #e2e8f0; border-radius: 12px; padding: 16px 20px; margin-bottom: 16px; page-break-inside: avoid; }
  .record-head { display: flex; align-items: baseline; gap: 10px; flex-wrap: wrap; }
  .record-head h2 { font-size: 16px; margin: 0; color: #0f172a; flex: 1; min-width: 120px; }
  .badge { font-size: 11px; font-weight: 700; background: #eff6ff; color: #1d4ed8; border-radius: 999px; padding: 2px 10px; }
  .date { font-size: 12px; color: #64748b; font-variant-numeric: tabular-nums; }
  .meta { font-size: 12px; color: #94a3b8; margin: 4px 0 10px; }
  .preview { max-width: 100%; max-height: 480px; display: block; margin: 8px 0; border: 1px solid #e2e8f0; border-radius: 8px; }
  figure.imaging { margin: 8px 0; }
  figure.imaging .preview { background: #000; margin-bottom: 4px; }
  figure.imaging .caption { font-size: 12px; color: #64748b; }
  .ocr-text { white-space: pre-wrap; word-break: break-word; font-size: 13px; line-height: 1.6; background: #f8fafc; border-radius: 8px; padding: 10px 12px; }
  .note { font-size: 12px; color: #94a3b8; font-style: italic; }
  .statement { text-align: center; font-size: 11px; color: #94a3b8; margin-top: 24px; padding-top: 12px; border-top: 1px solid #e2e8f0; }
  @media print {
    body { background: #fff; padding: 0; }
    .record { border: 1px solid #cbd5e1; box-shadow: none; }
    @page { margin: 16mm 14mm; }
  }
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::{NewDocument, NewOcr, OcrBackendKind};

    #[test]
    fn builds_html_with_escaped_text_and_records() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let imp = vault.import("血常规.txt", "text/plain", b"data").unwrap();
        let doc = vault
            .add_document(NewDocument {
                source_file_id: imp.source_file.id,
                doc_type: DocType::LabReport,
                doc_date: Some(chrono::Utc::now()),
                doc_date_end: None,
                title: Some("<b>血常规</b>".into()),
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
                text: "<script>alert(1)</script> 白细胞 10".into(),
                confidence: None,
            })
            .unwrap();

        let (html, count) =
            build_timeline_html(&vault, &crate::render_dicom_png_in_process).unwrap();
        assert_eq!(count, 1);
        // 转义生效:原始 <script> 标签不应逐字出现在输出里
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!html.contains("<script>alert"));
        assert!(html.contains("&lt;b&gt;血常规&lt;/b&gt;"));
        assert!(html.contains("白细胞"));
        assert!(html.contains("化验")); // 类型徽标
        assert!(html.contains("本导出由 MedMe 生成"));
        assert!(html.starts_with("<!doctype html>"));
    }

    #[test]
    fn ranged_export_filters_by_doc_date() {
        use chrono::TimeZone;
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let mk = |name: &str, date: Option<chrono::DateTime<chrono::Utc>>| {
            let imp = vault.import(name, "text/plain", name.as_bytes()).unwrap();
            vault
                .add_document(NewDocument {
                    source_file_id: imp.source_file.id,
                    doc_type: DocType::LabReport,
                    doc_date: date,
                    doc_date_end: None,
                    title: Some(name.into()),
                    language: None,
                    page_count: 1,
                })
                .unwrap();
        };
        let d = |y, m, day| chrono::Utc.with_ymd_and_hms(y, m, day, 12, 0, 0).unwrap();
        mk("old.txt", Some(d(2020, 1, 1)));
        mk("mid.txt", Some(d(2023, 6, 15)));
        mk("new.txt", Some(d(2025, 1, 1)));
        mk("nodate.txt", None);

        let r = &crate::render_dicom_png_in_process;
        // 全量(不筛选):4 份,含无日期
        let (_, all) = build_timeline_html_ranged(&vault, r, None, None).unwrap();
        assert_eq!(all, 4);
        // 区间 [2023-01-01, 2024-01-01]:只 mid 命中;无日期记录被排除
        let (html, n) =
            build_timeline_html_ranged(&vault, r, Some(d(2023, 1, 1)), Some(d(2024, 1, 1)))
                .unwrap();
        assert_eq!(n, 1);
        assert!(html.contains("mid.txt"));
        assert!(!html.contains("old.txt"));
        assert!(!html.contains("nodate.txt"));
        // 只给起点 2024-01-01:仅 new 命中
        let (_, from_only) =
            build_timeline_html_ranged(&vault, r, Some(d(2024, 1, 1)), None).unwrap();
        assert_eq!(from_only, 1);
    }

    #[test]
    fn imaging_export_embeds_anchor_slice() {
        // 影像检查文档导出应内嵌“关键切片” PNG(轻档),而非跳过或报错。
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

        let (html, count) =
            build_timeline_html(&vault, &crate::render_dicom_png_in_process).unwrap();
        assert_eq!(count, 1);
        // 关键切片 PNG 已内嵌,配“完整序列见分享”说明。
        assert!(html.contains("data:image/png;base64,"));
        assert!(html.contains("关键切片"));
    }

    /// Imports a DICOM instance and files it as an imaging document. Shared by the
    /// decoder-injection seam tests below.
    fn vault_with_one_dicom(dir: &std::path::Path) -> Vault {
        let vault = Vault::open(dir).unwrap();
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
        vault
    }

    #[test]
    fn dicom_preview_comes_only_from_the_injected_renderer() {
        // GHSA-24px seam: the export must obtain every DICOM preview PNG from the
        // injected renderer (which desktop points at an isolated subprocess), never
        // by calling an in-crate codec itself. Inject a sentinel renderer whose
        // bytes could not come from a real PNG encoder; if those exact bytes land in
        // the export, the decode went through the seam and nowhere else.
        let dir = tempfile::tempdir().unwrap();
        let vault = vault_with_one_dicom(dir.path());
        let sentinel: Vec<u8> = b"INJECTED-NOT-A-REAL-PNG".to_vec();
        let (html, count) = build_timeline_html(&vault, &|_| Some(sentinel.clone())).unwrap();
        assert_eq!(count, 1);
        assert!(
            html.contains(&format!("data:image/png;base64,{}", B64.encode(&sentinel))),
            "the injected renderer's bytes must be what the export embeds"
        );
    }

    #[test]
    fn dicom_export_degrades_when_injected_renderer_returns_none() {
        // Behavior preservation: an unsupported/failed decode (renderer → None, as a
        // crashed/killed subprocess reports) must degrade to the existing one-line
        // note, exactly like the pre-injection unsupported-transfer-syntax path —
        // never abort the whole export.
        let dir = tempfile::tempdir().unwrap();
        let vault = vault_with_one_dicom(dir.path());
        let (html, count) = build_timeline_html(&vault, &|_| None).unwrap();
        assert_eq!(count, 1);
        assert!(!html.contains("data:image/png;base64,"), "no PNG when None");
        assert!(
            html.contains("不支持的压缩格式"),
            "degrades to the text note"
        );
    }

    #[test]
    fn export_head_has_hardened_csp() {
        // The export is a static, script-less document; a hardened CSP must be present
        // so even a value that slipped past escaping cannot load remote resources or run
        // script.
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let (html, _) = build_timeline_html(&vault, &crate::render_dicom_png_in_process).unwrap();
        assert!(html.contains(
            "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src data:; style-src 'unsafe-inline'; script-src 'none'; form-action 'none'; base-uri 'none'\">"
        ));
    }

    /// The `data:{mime};base64,...` src is built with `escape_html(&sf.mime_type)`; a
    /// mime carrying a `"` (attribute-breakout attempt) must be neutralized to `&quot;`
    /// so it cannot start a new attribute or `<script>` tag.
    #[test]
    fn mime_type_cannot_break_out_of_img_src_attribute() {
        let evil = "image/png\"><script>alert(1)</script>";
        let escaped = escape_html(evil);
        assert!(!escaped.contains('"'), "no raw quote survives");
        assert!(!escaped.contains('<'), "no raw angle bracket survives");
        assert!(escaped.contains("&quot;"));
        assert!(escaped.contains("&lt;script&gt;"));
    }

    #[test]
    fn handles_empty_vault() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let (html, count) =
            build_timeline_html(&vault, &crate::render_dicom_png_in_process).unwrap();
        assert_eq!(count, 0);
        assert!(html.contains("本导出由 MedMe 生成"));
    }
}
