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

pub fn migrate(conn: &Connection) -> Result<(), MedmeError> {
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if v < 1 {
        conn.execute_batch(&format!(
            "BEGIN;\n{SCHEMA_V1}\nPRAGMA user_version = 1;\nCOMMIT;"
        ))?;
    }
    Ok(())
}
