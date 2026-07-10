import { invoke } from "@tauri-apps/api/core";
import type {
  TimelineGroup,
  ImportOutcome,
  ShareResult,
  PatientProfile,
  DocumentDetail,
  IcloudStatus,
} from "./types";

export const api = {
  loadArchive: () => invoke<TimelineGroup[]>("load_archive"),
  ingestFile: (path: string) => invoke<ImportOutcome>("ingest_file", { path }),
  // 相机/相册直传:传原始文件名 + 字节(数组),后端写临时文件再走 ingest。
  // iOS WKWebView 里选中的照片只有 File 对象、拿不到沙盒路径,故传字节而非路径。
  ingestBytes: (filename: string, data: number[]) =>
    invoke<ImportOutcome>("ingest_bytes", { filename, data }),
  getDocument: (id: number) => invoke<DocumentDetail>("get_document", { id }),
  readSourceBytes: (id: number) =>
    invoke<ArrayBuffer>("read_source_bytes", { id }),
  // 读取已生成的加密分享 .html 文件字节(前端据此构造 File 调起系统「分享」sheet)。
  readShareBytes: (path: string) =>
    invoke<ArrayBuffer>("read_share_bytes", { path }),
  getPatientProfile: () => invoke<PatientProfile>("get_patient_profile"),
  createShare: (expiresDays?: number) =>
    invoke<ShareResult>("create_share", { expiresDays }),
  loadDemoData: () => invoke<number>("load_demo_data"),
  getVaultPath: () => invoke<string>("get_vault_path"),
  resetVault: () => invoke<void>("reset_vault"),
  // iCloud 同步(仅 iOS)。status 供开关渲染;enable 迁移真相到 iCloud 容器。
  icloudStatus: () => invoke<IcloudStatus>("icloud_status"),
  enableIcloudSync: () => invoke<boolean>("enable_icloud_sync"),
};
