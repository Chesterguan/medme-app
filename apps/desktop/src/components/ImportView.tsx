import { useEffect, useState } from "react";
import { UploadCloud, ScanLine, FolderOpen, Inbox, Download, FileDown } from "lucide-react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { save } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import type { ImportOutcome } from "../types";

const STATUS_META: Record<string, { label: string; cls: string }> = {
  new: { label: "新增并索引", cls: "text-emerald-700 bg-emerald-50" },
  backfilled: { label: "补充索引", cls: "text-emerald-700 bg-emerald-50" },
  deduped: { label: "已存在 · 去重", cls: "text-slate-600 bg-slate-100" },
  stored_no_text: { label: "已保存 · 待 OCR", cls: "text-amber-700 bg-amber-50" },
};

export default function ImportView({ onImported }: { onImported: () => void }) {
  const [dragging, setDragging] = useState(false);
  const [busy, setBusy] = useState(false);
  const [results, setResults] = useState<ImportOutcome[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [inboxPath, setInboxPath] = useState<string | null>(null);
  const [exporting, setExporting] = useState(false);
  const [exportMsg, setExportMsg] = useState<
    { kind: "ok"; text: string; path: string } | { kind: "err"; text: string } | null
  >(null);

  useEffect(() => {
    api.getInboxPath().then(setInboxPath).catch(() => {});
  }, []);

  // 导出 v1:选保存路径 → 生成自包含 HTML → 浏览器可「打印 / 另存为 PDF」交给医生。
  const doExport = async () => {
    let path: string | null;
    try {
      path = await save({
        defaultPath: "MedMe导出.html",
        filters: [{ name: "HTML", extensions: ["html"] }],
      });
    } catch (e) {
      setExportMsg({ kind: "err", text: `选择保存位置失败:${String(e)}` });
      return;
    }
    if (!path) return;
    setExporting(true);
    setExportMsg(null);
    try {
      const summary = await api.exportTimelineHtml(path);
      setExportMsg({
        kind: "ok",
        text: `已导出 ${summary.file_count} 份记录,可在浏览器打开后「打印 / 另存为 PDF」交给医生。`,
        path,
      });
    } catch (e) {
      setExportMsg({ kind: "err", text: `导出失败:${String(e)}` });
    } finally {
      setExporting(false);
    }
  };

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        const p = event.payload;
        if (p.type === "enter" || p.type === "over") {
          setDragging(true);
        } else if (p.type === "leave") {
          setDragging(false);
        } else if (p.type === "drop") {
          setDragging(false);
          const paths = p.paths ?? [];
          if (paths.length) doImport(paths);
        }
      })
      .then((f) => {
        unlisten = f;
      });
    return () => {
      if (unlisten) unlisten();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const doImport = (paths: string[]) => {
    setBusy(true);
    setError(null);
    api
      .importPaths(paths)
      .then((r) => {
        setResults(r);
        onImported();
      })
      .catch((e) => setError(String(e)))
      .finally(() => setBusy(false));
  };

  return (
    <div className="flex-1 overflow-y-auto bg-slate-50 p-6 md:p-10">
      <div className="max-w-3xl mx-auto">
        <h1 className="text-2xl font-bold text-slate-900 mb-6">导入 · 导出</h1>

        <div
          className={`rounded-2xl border-2 border-dashed p-12 text-center transition-all ${
            dragging ? "border-blue-400 bg-blue-50" : "border-slate-300 bg-white"
          }`}
        >
          <UploadCloud
            className={`w-12 h-12 mx-auto mb-4 ${dragging ? "text-blue-500" : "text-slate-400"}`}
          />
          <div className="text-slate-700 font-medium">
            {busy ? "正在导入…" : dragging ? "松开以导入" : "把病历文件拖到这里"}
          </div>
          <div className="text-xs font-mono text-slate-400 mt-2">
            PDF · 图片(PNG / JPG / TIFF)· TXT · 原始文件永久保存,自动去重
          </div>
        </div>

        {/* 自动收件箱(Watch Folder):手机拍照云同步到这里即自动入库 */}
        <div className="mt-5 rounded-2xl border border-slate-200 bg-white p-5">
          <div className="flex items-center gap-2 text-slate-800 font-medium mb-2">
            <Inbox className="w-5 h-5 text-blue-500" /> 自动收件箱
          </div>
          <div className="text-sm text-slate-500 leading-relaxed mb-3">
            手机拍照存到这里(或其云同步目录)即自动入库,无需手动导入。
          </div>
          <div className="flex items-center justify-between gap-3 bg-slate-50 border border-slate-200 rounded-xl px-4 py-2.5">
            <span className="text-xs font-mono text-slate-600 truncate">
              {inboxPath ?? "加载中…"}
            </span>
            <button
              type="button"
              onClick={() => api.openInbox().catch((e) => setError(String(e)))}
              className="shrink-0 flex items-center gap-1.5 text-xs font-medium text-blue-700 bg-blue-50 hover:bg-blue-100 rounded-lg px-3 py-1.5 transition-colors"
            >
              <FolderOpen className="w-3.5 h-3.5" /> 打开收件箱文件夹
            </button>
          </div>
        </div>

        {/* 用户引导:怎样获得最准的识别 */}
        <div className="mt-5 rounded-2xl border border-blue-100 bg-blue-50/50 p-5">
          <div className="flex items-center gap-2 text-blue-800 font-medium mb-3">
            <ScanLine className="w-5 h-5" /> 怎样识别最准
          </div>
          <ul className="space-y-2.5 text-sm text-slate-600 leading-relaxed">
            <li className="flex gap-2">
              <span className="text-blue-500 font-bold shrink-0">①</span>
              <span>
                <b className="text-slate-800">优先用扫描 App</b>:扫描全能王 · 微信「扫一扫」文档模式 ·
                iOS 备忘录/文件扫描 —— 自动纠偏去阴影,识别最准,导出 PDF/图片后拖进来即可。
              </span>
            </li>
            <li className="flex gap-2">
              <span className="text-blue-500 font-bold shrink-0">②</span>
              <span>
                <b className="text-slate-800">直接拍照也行</b>:报告平铺填满画面、光线均匀、避免阴影反光、对焦清晰。
              </span>
            </li>
            <li className="flex gap-2">
              <span className="text-blue-500 font-bold shrink-0">③</span>
              <span>
                支持 <b className="text-slate-800">PDF · 图片 · 文本</b>;
                <b className="text-slate-800">原件永久保存、自动去重</b>,内容由 OCR 自动识别并归类到时间线。
              </span>
            </li>
          </ul>
        </div>

        {/* 导出(与导入同区,功能分开):全量时间线 → 自包含 HTML,浏览器打印/另存 PDF 交给医生 */}
        <div className="mt-5 rounded-2xl border border-slate-200 bg-white p-5">
          <div className="flex items-center gap-2 text-slate-800 font-medium mb-2">
            <FileDown className="w-5 h-5 text-blue-500" /> 导出给医生
          </div>
          <div className="text-sm text-slate-500 leading-relaxed mb-3">
            把全部病历按时间导出为一个自包含 HTML 文件,任意浏览器可打开、原生中文显示,
            再「打印 / 另存为 PDF」交给医生或用于报销。
          </div>
          <button
            type="button"
            onClick={doExport}
            disabled={exporting}
            className="flex items-center gap-2 text-sm font-medium text-white bg-blue-600 hover:bg-blue-700 disabled:opacity-50 disabled:cursor-wait rounded-xl px-4 py-2.5 transition-colors cursor-pointer"
          >
            <Download className="w-4 h-4" /> {exporting ? "导出中…" : "导出全部病历"}
          </button>
          {exportMsg && (
            <div
              className={`mt-3 rounded-xl px-4 py-2.5 text-sm leading-relaxed break-all ${
                exportMsg.kind === "ok"
                  ? "bg-emerald-50 text-emerald-700"
                  : "bg-rose-50 text-rose-700"
              }`}
            >
              <div>{exportMsg.text}</div>
              {exportMsg.kind === "ok" && (
                <button
                  onClick={() =>
                    api
                      .openPath(exportMsg.path)
                      .catch((e) => setExportMsg({ kind: "err", text: `打开失败:${String(e)}` }))
                  }
                  className="mt-1 font-medium text-blue-700 hover:underline cursor-pointer"
                >
                  打开文件
                </button>
              )}
            </div>
          )}
        </div>

        {error && <div className="mt-4 text-sm text-rose-600">导入失败:{error}</div>}

        {results.length > 0 && (
          <div className="mt-6 space-y-2">
            <div className="text-[11px] font-mono text-slate-400 uppercase tracking-widest">
              本次结果 · {results.length} 个文件
            </div>
            {results.map((r, i) => {
              const m = STATUS_META[r.status] ?? {
                label: r.status,
                cls: "text-slate-600 bg-slate-100",
              };
              return (
                <div
                  key={i}
                  className="flex items-center justify-between bg-white border border-slate-200 rounded-xl px-4 py-2.5"
                >
                  <span className="text-sm text-slate-700 truncate">{r.name}</span>
                  <span
                    className={`text-xs font-mono px-2 py-0.5 rounded-full shrink-0 ml-3 ${m.cls}`}
                  >
                    {m.label}
                  </span>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
