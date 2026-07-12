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
  ImagingInstance,
} from "./types";

export const api = {  listTimelineGrouped: () => invoke<TimelineGroup[]>("list_timeline_grouped"),
  search: (query: string, limit = 30) =>
    invoke<SearchResult[]>("search", { query, limit }),
  getDocument: (id: number) => invoke<DocumentDetail>("get_document", { id }),
  // 安全:文件选择器在 Rust 侧弹出并直接导入用户所选文件——路径绝不经由 webview 传入
  // (旧 import_paths 会信任任意前端路径,可被 XSS 用来读取任意文件)。返回逐个文件结果。
  importViaDialog: () => invoke<ImportOutcome[]>("import_via_dialog"),
  // 一键「加载示例数据」(张建国):导入随应用打包的 demo-data/,返回处理的文件数。
  loadDemoData: () => invoke<number>("load_demo_data"),
  // 后端用 tauri::ipc::Response 返回原始字节(而非 Vec<u8> 序列化成 JSON number[]),
  // invoke() 对应解析为 ArrayBuffer,避免大文件在 IPC 上被膨胀成文本。
  readSourceBytes: (id: number) => invoke<ArrayBuffer>("read_source_bytes", { id }),
  renderDicom: (id: number) => invoke<ArrayBuffer>("render_dicom", { id }),
  // 后端解码压缩 DICOM 帧(JPEG2000/JPEG-LS/RLE,轻量 JS 查看器解不了的格式)→ 原始像素。
  // 返回单个 ArrayBuffer:4 字节小端头长 + JSON 帧头 + 原始像素字节(见 DicomViewer 拆包)。
  decodeDicomFrame: (sourceFileId: number, frameIndex: number) =>
    invoke<ArrayBuffer>("decode_dicom_frame", { sourceFileId, frameIndex }),
  getImagingInstances: (documentId: number) =>
    invoke<ImagingInstance[]>("get_imaging_instances", { documentId }),
  // 安全:保存位置由 Rust 侧原生「保存」对话框选定并直接写入——路径绝不经由 webview
  // 传入(见 GHSA-gmg4)。写入后端返回含写入路径的结果;用户取消对话框返回 null。
  exportTimelineHtml: () =>
    invoke<ExportSummary | null>("export_timeline_html"),
  createShare: (expiresDays?: number) =>
    invoke<ShareResult | null>("create_share", { expiresDays }),
  getPatientProfile: () => invoke<PatientProfile>("get_patient_profile"),
  getInboxPath: () => invoke<string>("get_inbox_path"),
  // 收件箱文件夹由 Rust 侧原生「选择文件夹」对话框选择,返回新路径(取消则返回原路径)。
  setInboxPath: () => invoke<string>("set_inbox_path"),
  openInbox: () => invoke<void>("open_inbox"),
  openPath: (path: string) => invoke<void>("open_path", { path }),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  getVaultPath: () => invoke<string>("get_vault_path"),
  // 更换保险箱位置:Rust 侧弹原生「选择文件夹」对话框,把现有病历搬到所选目录
  //(指向云同步文件夹即可多设备同步),返回新路径(取消则返回原路径)。
  setVaultPath: () => invoke<string>("set_vault_path"),
  // 清空保险箱 · 重置(格式化):删掉当前保险箱内容并重建一个空的,用于清掉示例数据
  // 或让用户从头开始——尤其应在开启云盘同步前做,否则示例数据会被同步进云盘。
  resetVault: () => invoke<void>("reset_vault"),
  getAuditLog: () => invoke<AuditEntry[]>("get_audit_log"),
  // 导出审计清单 CSV:内容由前端按审计条目生成,后端写入固定导出目录并返回写入路径。
  exportAuditCsv: (contents: string) =>
    invoke<string>("export_audit_csv", { contents }),
};
