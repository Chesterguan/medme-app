use crate::MedmeError;
use rusqlite::Connection;

const SCHEMA_V1: &str = r#"
CREATE TABLE source_file (
    id            INTEGER PRIMARY KEY,
    content_hash  TEXT    NOT NULL UNIQUE,
    original_name TEXT    NOT NULL,
    mime_type     TEXT    NOT NULL,
    byte_size     INTEGER NOT NULL,
    storage_path  TEXT    NOT NULL,
    imported_at   TEXT    NOT NULL
);
CREATE TABLE document (
    id             INTEGER PRIMARY KEY,
    source_file_id INTEGER NOT NULL REFERENCES source_file(id),
    doc_type       TEXT    NOT NULL DEFAULT 'unknown',
    doc_date       TEXT,
    title          TEXT,
    language       TEXT,
    page_count     INTEGER NOT NULL DEFAULT 0,
    created_at     TEXT    NOT NULL,
    UNIQUE(source_file_id)
);
CREATE INDEX idx_document_date ON document(doc_date);
CREATE INDEX idx_document_type ON document(doc_type);
CREATE TABLE ocr_result (
    id            INTEGER PRIMARY KEY,
    document_id   INTEGER NOT NULL REFERENCES document(id) ON DELETE CASCADE,
    page_no       INTEGER NOT NULL,
    backend       TEXT    NOT NULL,
    model_version TEXT    NOT NULL,
    text          TEXT    NOT NULL,
    confidence    REAL,
    layout_json   TEXT,
    created_at    TEXT    NOT NULL,
    UNIQUE(document_id, page_no)
);
CREATE VIRTUAL TABLE document_fts USING fts5(
    title, body, document_id UNINDEXED,
    tokenize = 'unicode61 remove_diacritics 2'
);
"#;

/// Small key/value table for the event-sourcing watermark (`applied_seq`) and
/// `device_id`. Created outside the `user_version` migration ladder (rather
/// than as a new numbered migration) so it doesn't disturb the existing
/// `user_version` assertions in tests — it's an orthogonal, additive concern.
pub fn ensure_meta_table(conn: &Connection) -> Result<(), MedmeError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )?;
    Ok(())
}

pub fn migrate(conn: &Connection) -> Result<(), MedmeError> {
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if v < 1 {
        conn.execute_batch(&format!(
            "BEGIN;\n{SCHEMA_V1}\nPRAGMA user_version = 1;\nCOMMIT;"
        ))?;
    }
    if v < 2 {
        conn.execute_batch(
            "BEGIN;\nALTER TABLE document ADD COLUMN doc_date_end TEXT;\nPRAGMA user_version = 2;\nCOMMIT;",
        )?;
    }
    if v < 3 {
        conn.execute_batch(
            "BEGIN;\n\
             CREATE TABLE encounter (\
               id INTEGER PRIMARY KEY, kind TEXT NOT NULL, provider TEXT, \
               start_date TEXT, end_date TEXT, title TEXT, created_at TEXT NOT NULL);\n\
             ALTER TABLE document ADD COLUMN encounter_id INTEGER REFERENCES encounter(id);\n\
             CREATE INDEX idx_document_encounter ON document(encounter_id);\n\
             CREATE INDEX idx_encounter_start ON encounter(start_date);\n\
             PRAGMA user_version = 3;\n\
             COMMIT;",
        )?;
    }
    if v < 4 {
        conn.execute_batch(
            "BEGIN;\n\
             ALTER TABLE encounter ADD COLUMN transferred INTEGER NOT NULL DEFAULT 0;\n\
             PRAGMA user_version = 4;\n\
             COMMIT;",
        )?;
    }
    Ok(())
}

#[cfg(test)]
pub fn schema_v1_for_test() -> &'static str {
    SCHEMA_V1
}

#[cfg(test)]
mod tests {
    use crate::Vault;

    #[test]
    fn migration_is_v2_with_doc_date_end() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path()).unwrap();
        assert_eq!(v.user_version().unwrap(), 4);
        // 列存在且可空:round-trip 一个区间
        let imp = v.import("h.txt", "text/plain", b"stay").unwrap();
        let start = chrono::DateTime::parse_from_rfc3339("2023-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let end = chrono::DateTime::parse_from_rfc3339("2023-01-20T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let doc = v
            .add_document(crate::types::NewDocument {
                source_file_id: imp.source_file.id,
                doc_type: crate::DocType::DischargeSummary,
                doc_date: Some(start),
                doc_date_end: Some(end),
                title: Some("住院".into()),
                language: Some("zh".into()),
                page_count: 1,
            })
            .unwrap();
        let back = v.document_by_id(doc.id).unwrap().unwrap();
        assert_eq!(back.doc_date.unwrap(), start);
        assert_eq!(back.doc_date_end.unwrap(), end);
        let tl = v.timeline().unwrap();
        assert_eq!(tl[0].doc_date_end.unwrap(), end);
    }

    #[test]
    fn migrate_from_v1_adds_column() {
        // 模拟旧 v1 库:只建 v1 schema + user_version=1,再迁移到 v2
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(&format!(
            "BEGIN;\n{}\nPRAGMA user_version = 1;\nCOMMIT;",
            crate::schema::schema_v1_for_test()
        ))
        .unwrap();
        assert_eq!(
            conn.query_row::<i64, _, _>("PRAGMA user_version", [], |r| r.get(0))
                .unwrap(),
            1
        );
        crate::schema::migrate(&conn).unwrap();
        assert_eq!(
            conn.query_row::<i64, _, _>("PRAGMA user_version", [], |r| r.get(0))
                .unwrap(),
            4
        );
        // 新列可用
        conn.execute("INSERT INTO source_file (content_hash,original_name,mime_type,byte_size,storage_path,imported_at) VALUES ('h','n','m',1,'p','t')", []).unwrap();
        conn.execute("INSERT INTO document (source_file_id, doc_type, doc_date_end, created_at) VALUES (1,'unknown','2023-01-20T00:00:00Z','t')", []).unwrap();
    }
}
