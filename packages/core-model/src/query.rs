use crate::types::parse_dt;
use crate::{DocType, MedmeError, Vault};
use chrono::{DateTime, Utc};

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub document_id: i64,
    pub title: Option<String>,
    pub snippet: String,
}

#[derive(Debug, Clone)]
pub struct TimelineEntry {
    pub document_id: i64,
    pub doc_date: Option<DateTime<Utc>>,
    pub doc_type: DocType,
    pub title: Option<String>,
}

impl Vault {
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, MedmeError> {
        // 把每个 token 包成 FTS5 字面短语("...",内部引号翻倍),并丢弃纯标点 token,
        // 使 '-'/':'/'"'/'(' 等运算符字符被当作字面量,原始用户输入不会触发 FTS5 语法错误。
        let match_q: String = crate::tokenize::tokenize(query)
            .split_whitespace()
            .filter(|t| t.chars().any(|c| c.is_alphanumeric()))
            .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" ");
        if match_q.is_empty() {
            return Ok(vec![]);
        }
        let mut stmt = self.conn().prepare(
            "SELECT document_id, title, snippet(document_fts, 1, '[', ']', '…', 12) AS snip
             FROM document_fts WHERE document_fts MATCH ?1 LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![match_q, limit as i64], |r| {
            Ok(SearchHit {
                document_id: r.get(0)?,
                title: r.get(1)?, // FTS 里存的是分词后的 title;仅作展示提示
                snippet: r.get(2)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn timeline(&self) -> Result<Vec<TimelineEntry>, MedmeError> {
        let mut stmt = self.conn().prepare(
            "SELECT id, doc_date, doc_type, title FROM document
             ORDER BY doc_date IS NULL, doc_date DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            let date_s: Option<String> = r.get(1)?;
            Ok(TimelineEntry {
                document_id: r.get(0)?,
                doc_date: date_s.map(parse_dt),
                doc_type: DocType::from_str(&r.get::<_, String>(2)?),
                title: r.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use crate::types::{NewDocument, NewOcr};
    use crate::Vault;
    use crate::{DocType, OcrBackendKind};

    fn seed(v: &Vault, title: &str, text: &str, date: Option<&str>) {
        let imp = v.import(title, "text/plain", text.as_bytes()).unwrap();
        let doc_date = date.map(|d| {
            chrono::DateTime::parse_from_rfc3339(d)
                .unwrap()
                .with_timezone(&chrono::Utc)
        });
        let doc = v
            .add_document(NewDocument {
                source_file_id: imp.source_file.id,
                doc_type: DocType::LabReport,
                doc_date,
                title: Some(title.into()),
                language: Some("mixed".into()),
                page_count: 1,
            })
            .unwrap();
        v.add_ocr(NewOcr {
            document_id: doc.id,
            page_no: 1,
            backend: OcrBackendKind::Native,
            model_version: "text-layer".into(),
            text: text.into(),
            confidence: None,
        })
        .unwrap();
    }

    #[test]
    fn search_matches_chinese_and_english() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        seed(
            &v,
            "血常规",
            "肌酐 Creatinine 120 升高",
            Some("2023-05-01T00:00:00Z"),
        );
        seed(
            &v,
            "用药单",
            "美托洛尔 Metoprolol 25mg",
            Some("2024-01-02T00:00:00Z"),
        );

        assert_eq!(v.search("Creatinine", 10).unwrap().len(), 1);
        assert_eq!(v.search("肌酐", 10).unwrap().len(), 1);
        assert_eq!(v.search("Metoprolol", 10).unwrap().len(), 1);
        assert_eq!(v.search("nonexistent", 10).unwrap().len(), 0);
    }

    #[test]
    fn search_handles_fts5_special_chars_gracefully() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        seed(
            &v,
            "炎症",
            "C-reactive protein 反应蛋白 升高",
            Some("2023-05-01T00:00:00Z"),
        );

        // 连字符查询过去会报 Sqlite 错误;现在应正常命中
        let hits = v.search("C-reactive", 10).unwrap();
        assert_eq!(hits.len(), 1);
        // 杂散引号 / 冒号 / 括号:不得 panic 或返回 Err
        assert!(v.search("\"unterminated", 10).is_ok());
        assert!(v.search("col:val", 10).is_ok());
        assert!(v.search("a AND (b", 10).is_ok());
        // 纯标点:短路返回空
        assert!(v.search("---", 10).unwrap().is_empty());
    }

    #[test]
    fn timeline_orders_desc_nulls_last() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        seed(&v, "old", "a", Some("2023-05-01T00:00:00Z"));
        seed(&v, "new", "b", Some("2024-01-02T00:00:00Z"));
        seed(&v, "undated", "c", None);

        let t = v.timeline().unwrap();
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].title.as_deref(), Some("new"));
        assert_eq!(t[1].title.as_deref(), Some("old"));
        assert!(t[2].doc_date.is_none()); // NULL 最后
    }
}
