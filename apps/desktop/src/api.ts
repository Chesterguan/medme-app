import { invoke } from "@tauri-apps/api/core";
import type {
  DocumentSummary,
  SearchResult,
  DocumentDetail,
  ImportOutcome,
  ExportSummary,
  PatientProfile,
  TimelineGroup,
} from "./types";

export const api = {
  listTimeline: () => invoke<DocumentSummary[]>("list_timeline"),
  listTimelineGrouped: () => invoke<TimelineGroup[]>("list_timeline_grouped"),
  search: (query: string, limit = 30) =>
    invoke<SearchResult[]>("search", { query, limit }),
  getDocument: (id: number) => invoke<DocumentDetail>("get_document", { id }),
  importPaths: (paths: string[]) =>
    invoke<ImportOutcome[]>("import_paths", { paths }),
  readSourceBytes: (id: number) => invoke<number[]>("read_source_bytes", { id }),
  renderDicom: (id: number) => invoke<number[]>("render_dicom", { id }),
  exportVault: (destPath: string) =>
    invoke<ExportSummary>("export_vault", { destPath }),
  getPatientProfile: () => invoke<PatientProfile>("get_patient_profile"),
  getInboxPath: () => invoke<string>("get_inbox_path"),
  setInboxPath: (path: string) => invoke<void>("set_inbox_path", { path }),
  openInbox: () => invoke<void>("open_inbox"),
};
