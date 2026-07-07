export interface DocumentSummary {
  id: number;
  doc_type: string;
  doc_date: string | null; // RFC3339
  doc_date_end: string | null; // RFC3339
  title: string | null;
  page_count: number;
}
export interface SourceFileMeta {
  id: number;
  original_name: string;
  mime_type: string;
  byte_size: number;
  imported_at: string;
}
export interface SearchResult {
  document: DocumentSummary;
  snippet: string;
}
export interface DocumentDetail {
  document: DocumentSummary;
  source_file: SourceFileMeta;
  ocr_text: string;
  ocr_confidence: number | null;
  ocr_backend: string | null;
}
export interface ImportOutcome {
  name: string;
  source_file_id: number;
  status: string;
  doc_type: string | null;
}
export interface ExportSummary {
  file_count: number;
  byte_size: number;
}
export interface ShareResult {
  passphrase: string;
  record_count: number;
  byte_size: number;
}
export interface PatientProfile {
  name: string | null;
  gender: string | null;
  birth_date: string | null;
  age: string | null;
  record_count: number;
}
export interface EncounterSummary {
  id: number;
  kind: string; // inpatient | outpatient | emergency | exam
  provider: string | null;
  start_date: string | null;
  end_date: string | null;
  title: string | null;
  transferred: boolean;
  doc_count: number;
}
// list_timeline_grouped 返回的分组:就诊组 或 独立文档
export type TimelineGroup =
  | { group_type: "encounter"; encounter: EncounterSummary; docs: DocumentSummary[] }
  | { group_type: "document"; doc: DocumentSummary };

// 审计追踪条目(隐藏的「审计/管理员」视图):见 core-model::audit。
export interface AuditEntry {
  seq: number;
  timestamp: string; // RFC3339
  device_id: string;
  action: string; // 导入 | 导出 | 分享
  detail: string;
  sha256: string | null;
}
