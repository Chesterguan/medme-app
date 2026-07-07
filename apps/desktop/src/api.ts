import { invoke } from "@tauri-apps/api/core";
import type {
  SearchResult,
  DocumentDetail,
  ImportOutcome,
  ExportSummary,
  ShareResult,
  PatientProfile,
  TimelineGroup,
  AuditEntry,
} from "./types";

export const api = {  listTimelineGrouped: () => invoke<TimelineGroup[]>("list_timeline_grouped"),
  search: (query: string, limit = 30) =>
    invoke<SearchResult[]>("search", { query, limit }),
  getDocument: (id: number) => invoke<DocumentDetail>("get_document", { id }),
  importPaths: (paths: string[]) =>
    invoke<ImportOutcome[]>("import_paths", { paths }),
  // 后端用 tauri::ipc::Response 返回原始字节(而非 Vec<u8> 序列化成 JSON number[]),
  // invoke() 对应解析为 ArrayBuffer,避免大文件在 IPC 上被膨胀成文本。
  readSourceBytes: (id: number) => invoke<ArrayBuffer>("read_source_bytes", { id }),
  renderDicom: (id: number) => invoke<ArrayBuffer>("render_dicom", { id }),
  exportVault: (destPath: string) =>
    invoke<ExportSummary>("export_vault", { destPath }),
  exportTimelineHtml: (destPath: string) =>
    invoke<ExportSummary>("export_timeline_html", { destPath }),
  createShare: (destPath: string, expiresDays?: number) =>
    invoke<ShareResult>("create_share", { destPath, expiresDays }),
  getPatientProfile: () => invoke<PatientProfile>("get_patient_profile"),
  getInboxPath: () => invoke<string>("get_inbox_path"),
  setInboxPath: (path: string) => invoke<void>("set_inbox_path", { path }),
  openInbox: () => invoke<void>("open_inbox"),
  openPath: (path: string) => invoke<void>("open_path", { path }),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  getVaultPath: () => invoke<string>("get_vault_path"),
  getAuditLog: () => invoke<AuditEntry[]>("get_audit_log"),
  writeTextFile: (path: string, contents: string) =>
    invoke<void>("write_text_file", { path, contents }),
};
