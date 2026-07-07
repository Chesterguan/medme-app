//! 审计追踪:把不可变事件日志(`log/*.jsonl`)投影成一份扁平、可展示的清单。
//!
//! 与 `materialize.rs` 不同,这里不写任何 SQLite 表 —— `audit_log()` 每次都直接
//! 重放 `log/`,保证展示内容与「不可篡改的日志」严格一致(哪怕 medme.db 被删除
//! 重建,审计记录也不受影响)。给隐藏的「审计/管理员」视图用。

use crate::event::Event;
use crate::{MedmeError, Vault};

/// 日志里的一条写操作,投影成便于展示的扁平结构。
#[derive(Debug, Clone, PartialEq)]
pub struct AuditEntry {
    pub seq: i64,
    pub timestamp: String,
    pub device_id: String,
    /// 中文动作名:导入 / 导出 / 分享。
    pub action: String,
    /// 文件名或概要(如「timeline_html · 12 条记录」)。
    pub detail: String,
    /// 内容 sha256(可核验、防篡改),部分事件类型没有(目前均有)。
    pub sha256: Option<String>,
}

impl Vault {
    /// 记录一次导出(如时间线 HTML 导出)。`kind` 是导出类型标识(如
    /// `"timeline_html"`),`sha256` 是导出产物内容的哈希。
    pub fn record_export(
        &self,
        kind: &str,
        sha256: &str,
        record_count: i64,
    ) -> Result<(), MedmeError> {
        let at = Self::now_rfc3339();
        self.append_event(Event::ExportPerformed {
            at,
            kind: kind.to_string(),
            record_count,
            sha256: sha256.to_string(),
        })?;
        self.materialize()
    }

    /// 记录一次加密分享。`sha256` 是分享产物(加密后的自包含 HTML)内容的哈希。
    pub fn record_share(
        &self,
        sha256: &str,
        record_count: i64,
        expires: &str,
    ) -> Result<(), MedmeError> {
        let at = Self::now_rfc3339();
        self.append_event(Event::ShareCreated {
            at,
            record_count,
            sha256: sha256.to_string(),
            expires: expires.to_string(),
        })?;
        self.materialize()
    }

    /// 审计追踪:按时间倒序(最新在前)列出所有导入/导出/分享事件。
    /// `DocumentAdded`/`OcrAdded` 是内部派生写入(同一次导入的一部分),噪声大,
    /// 跳过不展示。
    pub fn audit_log(&self) -> Result<Vec<AuditEntry>, MedmeError> {
        let entries = self.log.read_all()?;
        let mut out: Vec<AuditEntry> = Vec::with_capacity(entries.len());
        for e in entries {
            let (action, detail, sha256): (&str, String, Option<String>) = match &e.event {
                Event::FileImported {
                    original_name,
                    content_hash,
                    ..
                } => ("导入", original_name.clone(), Some(content_hash.clone())),
                Event::ExportPerformed {
                    kind,
                    record_count,
                    sha256,
                    ..
                } => (
                    "导出",
                    format!("{kind} · {record_count} 条记录"),
                    Some(sha256.clone()),
                ),
                Event::ShareCreated {
                    record_count,
                    expires,
                    sha256,
                    ..
                } => (
                    "分享",
                    format!("{record_count} 条记录 · 有效期至 {expires}"),
                    Some(sha256.clone()),
                ),
                Event::DocumentAdded { .. } | Event::OcrAdded { .. } => continue,
            };
            out.push(AuditEntry {
                seq: e.seq,
                timestamp: e.ts.clone(),
                device_id: e.device_id.clone(),
                action: action.to_string(),
                detail,
                sha256,
            });
        }
        out.reverse(); // 日志本身是最旧在前(seq 升序)—— 展示要求最新在前
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{NewDocument, NewOcr};
    use crate::{DocType, OcrBackendKind};

    #[test]
    fn audit_log_includes_import_export_share_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();

        let imp = v.import("血常规.pdf", "application/pdf", b"data").unwrap();
        v.add_document(NewDocument {
            source_file_id: imp.source_file.id,
            doc_type: DocType::LabReport,
            doc_date: None,
            doc_date_end: None,
            title: Some("血常规".into()),
            language: None,
            page_count: 1,
        })
        .unwrap();

        v.record_export("timeline_html", "deadbeef", 3).unwrap();
        v.record_share("cafef00d", 3, "2099-01-01T00:00:00Z").unwrap();

        let log = v.audit_log().unwrap();
        // 只应看到 导入/导出/分享 三条,DocumentAdded 被跳过
        assert_eq!(log.len(), 3);
        // 最新在前:分享 → 导出 → 导入
        assert_eq!(log[0].action, "分享");
        assert_eq!(log[0].sha256.as_deref(), Some("cafef00d"));
        assert!(log[0].detail.contains("有效期至"));
        assert_eq!(log[1].action, "导出");
        assert_eq!(log[1].sha256.as_deref(), Some("deadbeef"));
        assert_eq!(log[2].action, "导入");
        assert_eq!(log[2].detail, "血常规.pdf");
        assert!(log[2].sha256.is_some());
    }

    #[test]
    fn rebuild_from_log_is_unaffected_by_audit_events() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();

        let imp = v.import("a.txt", "text/plain", b"hello world").unwrap();
        let doc = v
            .add_document(NewDocument {
                source_file_id: imp.source_file.id,
                doc_type: DocType::LabReport,
                doc_date: Some(chrono::Utc::now()),
                doc_date_end: None,
                title: Some("t".into()),
                language: None,
                page_count: 1,
            })
            .unwrap();
        v.add_ocr(NewOcr {
            document_id: doc.id,
            page_no: 1,
            backend: OcrBackendKind::Native,
            model_version: "text-layer".into(),
            text: "some ocr text".into(),
            confidence: None,
        })
        .unwrap();

        // 审计事件穿插在真实写操作之间/之后——这是最容易让 rebuild 翻车的排列。
        v.record_export("timeline_html", "hash1", 1).unwrap();
        v.record_share("hash2", 1, "2099-01-01T00:00:00Z").unwrap();

        let before_audit = v.audit_log().unwrap();
        let before_sf_count = v.debug_count("source_file");
        let before_doc_count = v.debug_count("document");
        let before_ocr_count = v.debug_count("ocr_result");

        // 关键风险点:rebuild_from_log 清空派生表后重放整条日志,必须能跳过
        // ExportPerformed/ShareCreated 而不 panic/报错,且不影响其余投影结果。
        v.rebuild_from_log().unwrap();

        assert_eq!(v.debug_count("source_file"), before_sf_count);
        assert_eq!(v.debug_count("document"), before_doc_count);
        assert_eq!(v.debug_count("ocr_result"), before_ocr_count);

        let after_audit = v.audit_log().unwrap();
        assert_eq!(after_audit.len(), before_audit.len());
        assert_eq!(after_audit, before_audit);
    }
}
