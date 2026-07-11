import { useEffect, useState } from "react";
import {
  UploadCloud,
  ScanLine,
  FolderOpen,
  Inbox,
  Download,
  FileDown,
  ShieldCheck,
  Copy,
  Check,
  Sparkles,
  Loader2,
} from "lucide-react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";
import type { ImportOutcome } from "../types";

const STATUS_META: Record<string, { label: string; cls: string }> = {
  new: { label: "新增并索引", cls: "text-emerald-700 bg-emerald-50" },
  backfilled: { label: "补充索引", cls: "text-emerald-700 bg-emerald-50" },
  deduped: { label: "已存在 · 去重", cls: "text-slate-600 bg-slate-100" },
  stored_no_text: { label: "已保存 · 待 OCR", cls: "text-amber-700 bg-amber-50" },
  instance_attached: { label: "已并入检查", cls: "text-sky-700 bg-sky-50" },
  failed: { label: "导入失败", cls: "text-rose-700 bg-rose-50" },
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

  // 一键「加载示例数据」(张建国):给刚装好 .dmg 的测试者一个免找文件的试用入口
  const [demoLoading, setDemoLoading] = useState(false);
  const [demoMsg, setDemoMsg] = useState<
    { kind: "ok"; text: string } | { kind: "err"; text: string } | null
  >(null);

  const doLoadDemo = async () => {
    setDemoLoading(true);
    setDemoMsg(null);
    try {
      const n = await api.loadDemoData();
      onImported();
      setDemoMsg({ kind: "ok", text: `已加载 ${n} 份示例记录,可在生命时间线查看。` });
    } catch (e) {
      setDemoMsg({ kind: "err", text: `加载示例数据失败:${String(e)}` });
    } finally {
      setDemoLoading(false);
    }
  };

  // 加密分享
  const [shareDays, setShareDays] = useState(5);
  const [sharing, setSharing] = useState(false);
  const [copied, setCopied] = useState(false);
  const [shareResult, setShareResult] = useState<
    | { kind: "ok"; passphrase: string; count: number; days: number; path: string }
    | { kind: "err"; text: string }
    | null
  >(null);

  // 端到端加密分享:后端(Rust)弹原生「保存」对话框选保存路径 → 生成自包含加密 HTML
  // (含浏览器内查看器)→ 返回口令(需另行单独告知医生)与写入路径。数据零服务器,
  // 浏览器本地解密。安全:保存路径由后端从原生对话框获得,不再经 webview 传入。
  const doShare = async () => {
    const days = Number.isFinite(shareDays) && shareDays > 0 ? Math.floor(shareDays) : 5;
    setSharing(true);
    setShareResult(null);
    setCopied(false);
    try {
      const r = await api.createShare(days);
      if (!r) return; // 用户取消了保存对话框
      setShareResult({
        kind: "ok",
        passphrase: r.passphrase,
        count: r.record_count,
        days,
        path: r.path,
      });
    } catch (e) {
      setShareResult({ kind: "err", text: `生成失败:${String(e)}` });
    } finally {
      setSharing(false);
    }
  };

  const copyPass = async (pass: string) => {
    try {
      await navigator.clipboard.writeText(pass);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      /* 剪贴板不可用时忽略 —— 用户可手动选择复制 */
    }
  };

  useEffect(() => {
    api.getInboxPath().then(setInboxPath).catch(() => {});
  }, []);

  // 导出 v1:后端(Rust)弹原生「保存」对话框选保存路径 → 生成自包含 HTML → 浏览器可
  // 「打印 / 另存为 PDF」交给医生。安全:保存路径由后端从原生对话框获得,不再经 webview 传入。
  const doExport = async () => {
    setExporting(true);
    setExportMsg(null);
    try {
      const summary = await api.exportTimelineHtml();
      if (!summary) return; // 用户取消了保存对话框
      setExportMsg({
        kind: "ok",
        text: `已导出 ${summary.file_count} 份记录,可在浏览器打开后「打印 / 另存为 PDF」交给医生。`,
        path: summary.path,
      });
    } catch (e) {
      setExportMsg({ kind: "err", text: `导出失败:${String(e)}` });
    } finally {
      setExporting(false);
    }
  };

  // 拖放:只用 webview 事件驱动高亮动画;实际导入由 Rust 侧的拖放处理器完成(它拿到的
  // 是 OS 可信路径),完成后发 `import-results` 事件带回逐个文件结果。安全:webview 不再
  // 把(可能被 XSS 伪造的)路径传给后端读取。
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
          if ((p.paths ?? []).length) {
            setBusy(true);
            setError(null);
          }
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

  // Rust 拖放处理器导入完成后带回的逐个文件结果(时间线刷新由 App 层的 vault-changed 负责)。
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen<ImportOutcome[]>("import-results", (event) => {
      setResults(event.payload);
      setBusy(false);
    }).then((f) => {
      unlisten = f;
    });
    return () => {
      if (unlisten) unlisten();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 「选择文件导入」:后端(Rust)弹出原生文件选择器并直接导入所选文件,返回逐个文件结果。
  const pickAndImport = async () => {
    setBusy(true);
    setError(null);
    try {
      const r = await api.importViaDialog();
      if (r.length) {
        setResults(r);
        onImported();
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex-1 overflow-y-auto bg-slate-50 p-6 md:p-10">
      <div className="max-w-3xl mx-auto">
        <h1 className="text-2xl font-bold text-slate-900 mb-6">导入 · 导出</h1>

        {/* 一键试用:免找文件,直接加载内置的张建国示例数据集 */}
        <div className="rounded-2xl border border-violet-200 bg-violet-50/50 p-5 mb-5">
          <div className="flex items-center gap-2 text-violet-800 font-medium mb-2">
            <Sparkles className="w-5 h-5" /> 一键试用
          </div>
          <div className="text-sm text-slate-600 leading-relaxed mb-3">
            还没有自己的病历?加载内置的<b className="text-slate-800">张建国</b>示例数据集
            (含检验报告、门诊病历、处方、影像检查等),立即体验完整的生命时间线。
            <br />
            <span className="text-xs text-slate-400">示例数据,可随时删除保险箱重来。</span>
          </div>
          <button
            type="button"
            onClick={doLoadDemo}
            disabled={demoLoading}
            className="flex items-center gap-2 text-sm font-medium text-white bg-violet-600 hover:bg-violet-700 disabled:opacity-50 disabled:cursor-wait rounded-xl px-4 py-2.5 transition-colors cursor-pointer"
          >
            {demoLoading ? (
              <Loader2 className="w-4 h-4 animate-spin" />
            ) : (
              <Sparkles className="w-4 h-4" />
            )}
            {demoLoading ? "加载中…" : "加载示例数据(张建国)"}
          </button>
          {demoMsg && (
            <div
              className={`mt-3 rounded-xl px-4 py-2.5 text-sm leading-relaxed break-all ${
                demoMsg.kind === "ok"
                  ? "bg-emerald-50 text-emerald-700"
                  : "bg-rose-50 text-rose-700"
              }`}
            >
              {demoMsg.text}
            </div>
          )}
        </div>

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
            PDF · 图片(PNG / JPG / TIFF / HEIC)· TXT · DICOM · 原始文件永久保存,自动去重
          </div>
          <div className="mt-5">
            <button
              type="button"
              onClick={pickAndImport}
              disabled={busy}
              className="inline-flex items-center gap-2 text-sm font-medium text-white bg-blue-600 hover:bg-blue-700 disabled:opacity-50 disabled:cursor-wait rounded-xl px-4 py-2.5 transition-colors cursor-pointer"
            >
              <FolderOpen className="w-4 h-4" /> 选择文件导入
            </button>
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

        {/* 加密分享给医生:端到端加密、零服务器,需口令打开 */}
        <div className="mt-5 rounded-2xl border border-slate-200 bg-white p-5">
          <div className="flex items-center gap-2 text-slate-800 font-medium mb-2">
            <ShieldCheck className="w-5 h-5 text-emerald-600" /> 加密分享给医生
          </div>
          <div className="text-sm text-slate-500 leading-relaxed mb-3">
            生成一个<b className="text-slate-700">端到端加密</b>的 HTML 文件(含浏览器内查看器):
            <b className="text-slate-700">零服务器</b>,医生用任意浏览器打开、输入<b className="text-slate-700">口令</b>即在本地解密查看,数据不上传任何服务器。
          </div>
          <div className="flex items-center gap-3 mb-1.5">
            <label className="text-sm text-slate-600">建议复阅期限</label>
            <input
              type="number"
              min={1}
              max={36500}
              value={shareDays}
              onChange={(e) => setShareDays(Number(e.target.value))}
              className="w-24 text-sm border border-slate-300 rounded-lg px-3 py-1.5 focus:outline-none focus:border-emerald-500"
            />
            <span className="text-sm text-slate-500">天</span>
          </div>
          <div className="text-xs text-slate-400 mb-3">
            仅作为给医生的复阅提醒,<b className="text-slate-500">并非强制</b>——文件本身不会到期失效,持有文件与口令者始终可解密查看。长期分享可设很大值(如 36500 天 ≈ 100 年)。
          </div>
          <button
            type="button"
            onClick={doShare}
            disabled={sharing}
            className="flex items-center gap-2 text-sm font-medium text-white bg-emerald-600 hover:bg-emerald-700 disabled:opacity-50 disabled:cursor-wait rounded-xl px-4 py-2.5 transition-colors cursor-pointer"
          >
            <ShieldCheck className="w-4 h-4" /> {sharing ? "生成中…" : "生成加密分享文件"}
          </button>

          {shareResult && shareResult.kind === "err" && (
            <div className="mt-3 rounded-xl px-4 py-2.5 text-sm bg-rose-50 text-rose-700 break-all">
              {shareResult.text}
            </div>
          )}
          {shareResult && shareResult.kind === "ok" && (
            <div className="mt-4 rounded-xl border border-emerald-200 bg-emerald-50/60 p-4">
              <div className="text-[11px] font-mono text-emerald-700 uppercase tracking-widest mb-1">
                口令(请务必单独告知医生)
              </div>
              <div className="flex items-center gap-2">
                <code className="flex-1 text-base font-mono font-semibold text-slate-900 bg-white border border-emerald-200 rounded-lg px-3 py-2 break-all select-all">
                  {shareResult.passphrase}
                </code>
                <button
                  type="button"
                  onClick={() => copyPass(shareResult.passphrase)}
                  className="shrink-0 flex items-center gap-1.5 text-xs font-medium text-emerald-700 bg-white border border-emerald-200 hover:bg-emerald-50 rounded-lg px-3 py-2 transition-colors cursor-pointer"
                >
                  {copied ? <Check className="w-3.5 h-3.5" /> : <Copy className="w-3.5 h-3.5" />}
                  {copied ? "已复制" : "复制"}
                </button>
              </div>
              <div className="mt-3 text-sm text-slate-600 leading-relaxed">
                已生成 {shareResult.count} 份记录。把文件发给医生(或存到你的云盘发链接),
                <b className="text-slate-800">口令请另行单独告知,切勿和文件放一起</b>。
                医生用任意浏览器打开 → 输口令 → 查看。建议复阅期限 {shareResult.days} 天(仅为提醒,非强制,文件不会自动失效)。
              </div>
              <button
                type="button"
                onClick={() =>
                  api
                    .openPath(shareResult.path)
                    .catch((e) => setShareResult({ kind: "err", text: `打开失败:${String(e)}` }))
                }
                className="mt-2 text-sm font-medium text-emerald-700 hover:underline cursor-pointer"
              >
                打开文件
              </button>
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
