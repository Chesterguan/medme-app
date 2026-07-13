use crate::types::{parse_dt, Document, Encounter, EncounterKind, SourceFile};
use crate::{DocType, MedmeError, Vault};
use chrono::{DateTime, Utc};
use rusqlite::OptionalExtension;

/// Column list shared by `document_by_id` and `documents_where` — keep order aligned
/// with the `Document` struct field order used when building rows.
const DOCUMENT_COLUMNS: &str = "id, source_file_id, doc_type, doc_date, doc_date_end, title, language, page_count, encounter_id, created_at";

fn row_to_document(r: &rusqlite::Row) -> rusqlite::Result<Document> {
    Ok(Document {
        id: r.get(0)?,
        source_file_id: r.get(1)?,
        doc_type: DocType::from_str(&r.get::<_, String>(2)?),
        doc_date: r.get::<_, Option<String>>(3)?.map(parse_dt),
        doc_date_end: r.get::<_, Option<String>>(4)?.map(parse_dt),
        title: r.get(5)?,
        language: r.get(6)?,
        page_count: r.get(7)?,
        encounter_id: r.get(8)?,
        created_at: parse_dt(r.get::<_, String>(9)?),
    })
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub document_id: i64,
    pub title: Option<String>,
    pub snippet: String,
}

/// 从文本抽取医院/医学中心名(2-18 个中文字,以 医院/医学中心 结尾)。取第一个匹配。
pub fn extract_provider(text: &str) -> Option<String> {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"([\x{4e00}-\x{9fa5}]{2,18}?(?:医院|医学中心))").expect("provider regex")
    });
    re.captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// 给定一组文档 id,返回各文档 OCR 文本命中的 provider 名(未去重,用于统计众数)。
fn providers_for_doc_ids(
    conn: &rusqlite::Connection,
    ids: &[i64],
) -> Result<Vec<String>, MedmeError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("SELECT text FROM ocr_result WHERE document_id IN ({placeholders})");
    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|i| i as &dyn rusqlite::ToSql).collect();
    let rows = stmt.query_map(params.as_slice(), |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for r in rows {
        let text = r?;
        if let Some(p) = extract_provider(&text) {
            out.push(p);
        }
    }
    Ok(out)
}

/// 组内 provider 众数(第一个达到最高频次的);transferred = 是否出现 ≥2 家不同医院。
fn provider_summary(providers: &[String]) -> (Option<String>, bool) {
    use std::collections::HashMap;
    let mut order: Vec<&String> = Vec::new();
    let mut counts: HashMap<&String, usize> = HashMap::new();
    for p in providers {
        if !counts.contains_key(p) {
            order.push(p);
        }
        *counts.entry(p).or_insert(0) += 1;
    }
    let transferred = order.len() >= 2;
    let mut best: Option<&String> = None;
    let mut best_count = 0usize;
    for p in &order {
        let c = counts[*p];
        if c > best_count {
            best_count = c;
            best = Some(p);
        }
    }
    (best.cloned(), transferred)
}

#[derive(Debug, Clone)]
pub struct TimelineEntry {
    pub document_id: i64,
    pub doc_date: Option<DateTime<Utc>>,
    pub doc_date_end: Option<DateTime<Utc>>,
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
            "SELECT id, doc_date, doc_date_end, doc_type, title FROM document
             ORDER BY doc_date IS NULL, doc_date DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            let date_s: Option<String> = r.get(1)?;
            let date_end_s: Option<String> = r.get(2)?;
            Ok(TimelineEntry {
                document_id: r.get(0)?,
                doc_date: date_s.map(parse_dt),
                doc_date_end: date_end_s.map(parse_dt),
                doc_type: DocType::from_str(&r.get::<_, String>(3)?),
                title: r.get(4)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 该 source_file 是否已建立 document(用于判断是否需要补索引)。
    pub fn has_document(&self, source_file_id: i64) -> Result<bool, MedmeError> {
        let n: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM document WHERE source_file_id = ?1",
            [source_file_id],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    pub fn document_by_id(&self, id: i64) -> Result<Option<Document>, MedmeError> {
        let row = self
            .conn()
            .query_row(
                &format!("SELECT {DOCUMENT_COLUMNS} FROM document WHERE id = ?1"),
                [id],
                row_to_document,
            )
            .optional()?;
        Ok(row)
    }

    /// 复用 `document_by_id` 的列顺序;`cond` 是不带 WHERE 的谓词片段(如 "encounter_id = ?1")。
    pub(crate) fn documents_where(
        &self,
        cond: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<Document>, MedmeError> {
        let mut stmt = self.conn().prepare(&format!(
            "SELECT {DOCUMENT_COLUMNS} FROM document WHERE {cond}
             ORDER BY doc_date IS NULL, doc_date DESC, id DESC"
        ))?;
        let rows = stmt.query_map(params, row_to_document)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Look up the (unique, per v0.1) document for a source file — used by
    /// `add_document` to return the materialized row after appending its event,
    /// and by the mobile FFI to report the ingested document id (review queue).
    pub fn document_by_source_file_id(
        &self,
        source_file_id: i64,
    ) -> Result<Option<Document>, MedmeError> {
        Ok(self
            .documents_where("source_file_id = ?1", &[&source_file_id])?
            .into_iter()
            .next())
    }

    pub fn rebuild_encounters(&self) -> Result<(), MedmeError> {
        use std::collections::HashSet;
        let tx = self.conn().unchecked_transaction()?;
        tx.execute("UPDATE document SET encounter_id = NULL", [])?;
        tx.execute("DELETE FROM encounter", [])?;
        // load docs sorted by doc_date (NULLs last)
        // (id, doc_type, doc_date, doc_date_end, title)
        type DocRow = (i64, String, Option<String>, Option<String>, Option<String>);
        let docs: Vec<DocRow> = {
            let mut stmt = tx.prepare(
                "SELECT id, doc_type, doc_date, doc_date_end, title FROM document
                 ORDER BY doc_date IS NULL, doc_date ASC, id ASC",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?;
            let mut v = Vec::new();
            for x in rows {
                v.push(x?);
            }
            v
        };
        let now = Self::now_rfc3339();
        let mut assigned: HashSet<i64> = HashSet::new();

        // helper: parse rfc3339 -> DateTime
        let parse = |s: &Option<String>| {
            s.as_ref()
                .and_then(|x| chrono::DateTime::parse_from_rfc3339(x).ok())
                .map(|d| d.with_timezone(&chrono::Utc))
        };

        // 1) 住院:每个 discharge_summary 带区间 → inpatient 窗;区间内文档归入
        for (id, dtype, dd, dde, _t) in &docs {
            if dtype != "discharge_summary" {
                continue;
            }
            let (Some(start), Some(end)) = (parse(dd), parse(dde)) else {
                continue;
            };
            let _ = id;
            // 先收集区间内(且未被更早住院窗占用)的文档 id,再统计 provider,最后一次性写入
            let mut member_ids: Vec<i64> = Vec::new();
            for (id2, _dt2, dd2, _dde2, _t2) in &docs {
                if assigned.contains(id2) {
                    continue;
                }
                if let Some(date2) = parse(dd2) {
                    if date2 >= start && date2 <= end {
                        member_ids.push(*id2);
                        assigned.insert(*id2);
                    }
                }
            }
            let providers = providers_for_doc_ids(&tx, &member_ids)?;
            let (provider, transferred) = provider_summary(&providers);
            let mut title = format!(
                "住院 · {} → {}",
                start.format("%Y-%m-%d"),
                end.format("%Y-%m-%d")
            );
            if transferred {
                title.push_str(" · 转院");
            }
            tx.execute(
                "INSERT INTO encounter (kind, provider, start_date, end_date, title, transferred, created_at) VALUES ('inpatient', ?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![provider, start.to_rfc3339(), end.to_rfc3339(), title, transferred, now],
            )?;
            let enc_id = tx.last_insert_rowid();
            for id2 in &member_ids {
                tx.execute(
                    "UPDATE document SET encounter_id = ?1 WHERE id = ?2",
                    rusqlite::params![enc_id, id2],
                )?;
            }
        }

        // 2) 同日聚合:剩余有日期文档按天分组
        use std::collections::BTreeMap;
        let mut byday: BTreeMap<String, Vec<(i64, bool)>> = BTreeMap::new(); // day -> (doc_id, is_emergency_by_title)
        for (id, _dt, dd, _dde, title) in &docs {
            if assigned.contains(id) {
                continue;
            }
            let Some(date) = parse(dd) else {
                continue;
            };
            let day = date.format("%Y-%m-%d").to_string();
            let emerg = title
                .as_deref()
                .map(|t| t.contains("急诊"))
                .unwrap_or(false);
            byday.entry(day).or_default().push((*id, emerg));
        }
        for (day, group) in byday {
            let emergency = group.iter().any(|(_, e)| *e);
            let kind = if emergency { "emergency" } else { "outpatient" };
            let label = if emergency { "急诊" } else { "门诊" };
            let start = format!("{day}T00:00:00+00:00");
            let member_ids: Vec<i64> = group.iter().map(|(id, _)| *id).collect();
            let providers = providers_for_doc_ids(&tx, &member_ids)?;
            let (provider, transferred) = provider_summary(&providers);
            let mut title = format!("{label} · {day}");
            if transferred {
                title.push_str(" · 转院");
            }
            tx.execute(
                "INSERT INTO encounter (kind, provider, start_date, end_date, title, transferred, created_at) VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6)",
                rusqlite::params![kind, provider, start, title, transferred, now],
            )?;
            let enc_id = tx.last_insert_rowid();
            for id in member_ids {
                tx.execute(
                    "UPDATE document SET encounter_id = ?1 WHERE id = ?2",
                    rusqlite::params![enc_id, id],
                )?;
            }
        }
        // 3) 无日期文档保持 encounter_id NULL
        tx.commit()?;
        Ok(())
    }

    pub fn encounters_with_docs(&self) -> Result<Vec<(Encounter, Vec<Document>)>, MedmeError> {
        let mut stmt = self.conn().prepare(
            "SELECT id, kind, provider, start_date, end_date, title, transferred, created_at FROM encounter
             ORDER BY start_date IS NULL, start_date DESC, id DESC",
        )?;
        let encs: Vec<Encounter> = stmt
            .query_map([], |r| {
                Ok(Encounter {
                    id: r.get(0)?,
                    kind: EncounterKind::from_str(&r.get::<_, String>(1)?),
                    provider: r.get(2)?,
                    start_date: r.get::<_, Option<String>>(3)?.map(parse_dt),
                    end_date: r.get::<_, Option<String>>(4)?.map(parse_dt),
                    title: r.get(5)?,
                    transferred: r.get::<_, i64>(6)? != 0,
                    created_at: parse_dt(r.get::<_, String>(7)?),
                })
            })?
            .collect::<Result<_, _>>()?;
        let mut out = Vec::new();
        for e in encs {
            let docs = self.documents_where("encounter_id = ?1", &[&e.id])?;
            out.push((e, docs));
        }
        Ok(out)
    }

    pub fn standalone_documents(&self) -> Result<Vec<Document>, MedmeError> {
        self.documents_where("encounter_id IS NULL", &[])
    }

    pub fn source_file_by_id(&self, id: i64) -> Result<Option<SourceFile>, MedmeError> {
        let row = self
            .conn()
            .query_row(
                "SELECT id, content_hash, original_name, mime_type, byte_size, storage_path, imported_at
                 FROM source_file WHERE id = ?1",
                [id],
                |r| {
                    Ok(SourceFile {
                        id: r.get(0)?,
                        content_hash: r.get(1)?,
                        original_name: r.get(2)?,
                        mime_type: r.get(3)?,
                        byte_size: r.get(4)?,
                        storage_path: r.get(5)?,
                        imported_at: parse_dt(r.get::<_, String>(6)?),
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    pub fn ocr_text(&self, document_id: i64) -> Result<String, MedmeError> {
        let mut stmt = self
            .conn()
            .prepare("SELECT text FROM ocr_result WHERE document_id = ?1 ORDER BY page_no ASC")?;
        let rows = stmt.query_map([document_id], |r| r.get::<_, String>(0))?;
        let mut parts = Vec::new();
        for r in rows {
            parts.push(r?);
        }
        Ok(parts.join("\n"))
    }

    /// 所有 OCR 文本(用于派生病人档案等跨文档聚合)。
    pub fn all_ocr_texts(&self) -> Result<Vec<String>, MedmeError> {
        let mut stmt = self.conn().prepare("SELECT text FROM ocr_result")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 文档的 OCR 置信度:取各页非空 confidence 的最小值(最保守 —— 有一页差就
    /// 提示)。若所有页均无 confidence(如 native 文本层文档),返回 None。
    pub fn ocr_confidence(&self, document_id: i64) -> Result<Option<f32>, MedmeError> {
        let v: Option<f32> = self.conn().query_row(
            "SELECT MIN(confidence) FROM ocr_result WHERE document_id = ?1 AND confidence IS NOT NULL",
            [document_id],
            |r| r.get(0),
        )?;
        Ok(v)
    }

    /// 文档的 OCR 后端(如 "onnx"/"native"/"vlm"):取该文档 ocr_result 的第一条
    /// 记录(按 page_no)。无 ocr_result 行时返回 None。
    pub fn ocr_backend(&self, document_id: i64) -> Result<Option<String>, MedmeError> {
        let row = self
            .conn()
            .query_row(
                "SELECT backend FROM ocr_result WHERE document_id = ?1 ORDER BY page_no ASC LIMIT 1",
                [document_id],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        Ok(row)
    }
}

#[cfg(test)]
mod tests {
    use crate::types::{NewDocument, NewOcr};
    use crate::Vault;
    use crate::{DocType, EncounterKind, OcrBackendKind};

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
                doc_date_end: None,
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

    #[test]
    fn reads_document_source_and_ocr_text() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        seed(
            &v,
            "血常规",
            "肌酐 Creatinine 120",
            Some("2023-05-01T00:00:00Z"),
        );

        let doc = v.timeline().unwrap()[0].clone();
        let full = v.document_by_id(doc.document_id).unwrap().unwrap();
        assert_eq!(full.title.as_deref(), Some("血常规"));

        let sf = v.source_file_by_id(full.source_file_id).unwrap().unwrap();
        assert_eq!(sf.original_name, "血常规");

        let text = v.ocr_text(doc.document_id).unwrap();
        assert!(text.contains("Creatinine"));

        // 不存在的 id → None / 空
        assert!(v.document_by_id(99999).unwrap().is_none());
        assert!(v.source_file_by_id(99999).unwrap().is_none());
        assert_eq!(v.ocr_text(99999).unwrap(), "");
    }

    #[test]
    fn ocr_confidence_is_min_across_pages_and_backend_is_first_page() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        let imp = v.import("scan.png", "image/png", b"fake-bytes").unwrap();
        let doc = v
            .add_document(NewDocument {
                source_file_id: imp.source_file.id,
                doc_type: DocType::LabReport,
                doc_date: None,
                doc_date_end: None,
                title: Some("scan.png".into()),
                language: None,
                page_count: 2,
            })
            .unwrap();
        v.add_ocr(NewOcr {
            document_id: doc.id,
            page_no: 1,
            backend: OcrBackendKind::Onnx,
            model_version: "ppocr-v5".into(),
            text: "page one".into(),
            confidence: Some(0.92),
        })
        .unwrap();
        v.add_ocr(NewOcr {
            document_id: doc.id,
            page_no: 2,
            backend: OcrBackendKind::Onnx,
            model_version: "ppocr-v5".into(),
            text: "page two, blurry".into(),
            confidence: Some(0.41),
        })
        .unwrap();

        // 最保守:取各页最小值,而非平均。
        assert_eq!(v.ocr_confidence(doc.id).unwrap(), Some(0.41));
        assert_eq!(v.ocr_backend(doc.id).unwrap(), Some("onnx".to_string()));

        // 无 OCR 行(如 native/无文本层)→ None。
        assert_eq!(v.ocr_confidence(99999).unwrap(), None);
        assert_eq!(v.ocr_backend(99999).unwrap(), None);

        // 全部 confidence 均为 NULL(如 native 文本层文档)→ None。
        let imp2 = v.import("native.txt", "text/plain", b"hello").unwrap();
        let doc2 = v
            .add_document(NewDocument {
                source_file_id: imp2.source_file.id,
                doc_type: DocType::Unknown,
                doc_date: None,
                doc_date_end: None,
                title: Some("native.txt".into()),
                language: None,
                page_count: 1,
            })
            .unwrap();
        v.add_ocr(NewOcr {
            document_id: doc2.id,
            page_no: 1,
            backend: OcrBackendKind::Native,
            model_version: "text-layer".into(),
            text: "hello".into(),
            confidence: None,
        })
        .unwrap();
        assert_eq!(v.ocr_confidence(doc2.id).unwrap(), None);
        assert_eq!(v.ocr_backend(doc2.id).unwrap(), Some("native".to_string()));
    }

    #[test]
    fn has_document_reflects_indexing() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        let imp = v.import("x.txt", "text/plain", b"hello").unwrap();
        assert!(!v.has_document(imp.source_file.id).unwrap()); // 存了但未建 document
        v.add_document(crate::types::NewDocument {
            source_file_id: imp.source_file.id,
            doc_type: crate::DocType::Unknown,
            doc_date: None,
            doc_date_end: None,
            title: None,
            language: None,
            page_count: 1,
        })
        .unwrap();
        assert!(v.has_document(imp.source_file.id).unwrap());
    }

    #[test]
    fn rebuild_groups_by_time() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        // 住院:入院-出院区间 + 区间内一份化验
        let d = |s: &str| {
            chrono::DateTime::parse_from_rfc3339(s)
                .unwrap()
                .with_timezone(&chrono::Utc)
        };
        let mk = |v: &Vault, dt: DocType, start: &str, end: Option<&str>, title: &str| {
            let imp = v.import(title, "text/plain", title.as_bytes()).unwrap();
            v.add_document(crate::types::NewDocument {
                source_file_id: imp.source_file.id,
                doc_type: dt,
                doc_date: Some(d(start)),
                doc_date_end: end.map(d),
                title: Some(title.into()),
                language: None,
                page_count: 1,
            })
            .unwrap()
            .id
        };
        mk(
            &v,
            DocType::DischargeSummary,
            "2023-04-24T00:00:00Z",
            Some("2023-05-01T00:00:00Z"),
            "出院记录",
        );
        mk(
            &v,
            DocType::LabReport,
            "2023-04-26T00:00:00Z",
            None,
            "住院期间化验",
        );
        mk(
            &v,
            DocType::LabReport,
            "2024-01-15T00:00:00Z",
            None,
            "门诊化验a",
        );
        mk(
            &v,
            DocType::ImagingReport,
            "2024-01-15T00:00:00Z",
            None,
            "门诊影像b",
        );
        v.rebuild_encounters().unwrap();

        let groups = v.encounters_with_docs().unwrap();
        // 住院组含 2 份(出院记录 + 区间内化验),门诊组含同日 2 份
        let inpatient = groups
            .iter()
            .find(|(e, _)| e.kind == EncounterKind::Inpatient)
            .unwrap();
        assert_eq!(inpatient.1.len(), 2);
        let outpatient = groups
            .iter()
            .find(|(e, _)| e.kind == EncounterKind::Outpatient)
            .unwrap();
        assert_eq!(outpatient.1.len(), 2);
        assert!(v.standalone_documents().unwrap().is_empty());
    }

    #[test]
    fn rebuild_marks_transfer_across_providers_in_inpatient_window() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        let d = |s: &str| {
            chrono::DateTime::parse_from_rfc3339(s)
                .unwrap()
                .with_timezone(&chrono::Utc)
        };
        let mk =
            |v: &Vault, dt: DocType, start: &str, end: Option<&str>, title: &str, text: &str| {
                let imp = v.import(title, "text/plain", title.as_bytes()).unwrap();
                let doc = v
                    .add_document(crate::types::NewDocument {
                        source_file_id: imp.source_file.id,
                        doc_type: dt,
                        doc_date: Some(d(start)),
                        doc_date_end: end.map(d),
                        title: Some(title.into()),
                        language: None,
                        page_count: 1,
                    })
                    .unwrap();
                v.add_ocr(crate::types::NewOcr {
                    document_id: doc.id,
                    page_no: 1,
                    backend: crate::OcrBackendKind::Native,
                    model_version: "text-layer".into(),
                    text: text.into(),
                    confidence: None,
                })
                .unwrap();
                doc.id
            };
        // 住院窗:两份文档来自不同医院 → 转院
        mk(
            &v,
            DocType::DischargeSummary,
            "2023-04-24T00:00:00Z",
            Some("2023-05-01T00:00:00Z"),
            "出院记录",
            "北京协和医院 出院记录",
        );
        mk(
            &v,
            DocType::LabReport,
            "2023-04-26T00:00:00Z",
            None,
            "住院期间化验",
            "上海华山医院 化验单",
        );
        v.rebuild_encounters().unwrap();

        let groups = v.encounters_with_docs().unwrap();
        let (inpatient, _) = groups
            .iter()
            .find(|(e, _)| e.kind == EncounterKind::Inpatient)
            .unwrap();
        assert!(inpatient.transferred, "should be marked as transferred");
        assert!(
            inpatient.provider.as_deref() == Some("北京协和医院")
                || inpatient.provider.as_deref() == Some("上海华山医院"),
            "provider should be one of the two hospitals, got {:?}",
            inpatient.provider
        );
        assert!(
            inpatient.title.as_deref().unwrap_or("").contains("转院"),
            "title should note 转院, got {:?}",
            inpatient.title
        );
    }

    #[test]
    fn rebuild_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        let imp = v.import("门诊化验", "text/plain", b"x").unwrap();
        v.add_document(crate::types::NewDocument {
            source_file_id: imp.source_file.id,
            doc_type: DocType::LabReport,
            doc_date: Some(
                chrono::DateTime::parse_from_rfc3339("2024-01-15T00:00:00Z")
                    .unwrap()
                    .with_timezone(&chrono::Utc),
            ),
            doc_date_end: None,
            title: Some("门诊化验".into()),
            language: None,
            page_count: 1,
        })
        .unwrap();
        v.rebuild_encounters().unwrap();
        v.rebuild_encounters().unwrap(); // 再来一次不应重复
        let n: i64 = v.debug_count("encounter");
        assert_eq!(n, 1);
    }
}
