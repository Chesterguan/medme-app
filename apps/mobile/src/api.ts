import { invoke } from "@tauri-apps/api/core";
import type {
  TimelineGroup,
  ImportOutcome,
  ShareResult,
  PatientProfile,
  DocumentDetail,
} from "./types";

export const api = {
  loadArchive: () => invoke<TimelineGroup[]>("load_archive"),
  ingestFile: (path: string) => invoke<ImportOutcome>("ingest_file", { path }),
  getDocument: (id: number) => invoke<DocumentDetail>("get_document", { id }),
  readSourceBytes: (id: number) =>
    invoke<ArrayBuffer>("read_source_bytes", { id }),
  getPatientProfile: () => invoke<PatientProfile>("get_patient_profile"),
  createShare: (expiresDays?: number) =>
    invoke<ShareResult>("create_share", { expiresDays }),
  loadDemoData: () => invoke<number>("load_demo_data"),
  getVaultPath: () => invoke<string>("get_vault_path"),
  resetVault: () => invoke<void>("reset_vault"),
};
