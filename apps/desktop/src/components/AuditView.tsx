import { useEffect, useState } from "react";
import { ShieldAlert, ArrowLeft, Copy, Check, FileDown } from "lucide-react";
import { save } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import type { AuditEntry } from "../types";

const ACTION_BADGE: Record<string, string> = {
  导入: "bg-emerald-50 text-emerald-700",
  导出: "bg-blue-50 text-blue-700",
  分享: "bg-amber-50 text-amber-700",
};

function fmtTs(ts: string): string {
  // RFC3339 → 本地可读时间,解析失败时原样展示。
  const d = new Date(ts);
  return Number.isNaN(d.getTime()) ? ts : d.toLocaleString();
}

function shortHash(h: string | null): string {
  if (!h) return "—";
  return h.length > 16 ? `${h.slice(0, 8)}…${h.slice(-6)}` : h;
}

export default function AuditView({ onNav }: { onNav: (id: string) => void }) {
  const [entries, setEntries] = useState<AuditEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [copiedSeq, setCopiedSeq] = useState<number | null>(null);

  useEffect(() => {
    api.getAuditLog().then(setEntries).catch((e) => setError(String(e)));
  }, []);

  const copyHash = async (seq: number, hash: string) => {
    try {
      await navigator.clipboard.writeText(hash);
      setCopiedSeq(seq);
      setTimeout(() => setCopiedSeq(null), 1500);
    } catch {
      /* 剪贴板不可用时忽略 */
    }
  };

  const exportManifest = async () => {
    const path = await save({
      defaultPath: "MedMe审计清单.csv",
      filters: [{ name: "CSV", extensions: ["csv"] }],
    }).catch(() => null);
    if (!path) return;
    const header = "seq,timestamp,device_id,action,detail,sha256\n";
    const rows = entries
      .map((e) =>
        [e.seq, e.timestamp, e.device_id, e.action, e.detail, e.sha256 ?? ""]
          .map((v) => `"${String(v).replace(/"/g, '""')}"`)
          .join(","),
      )
      .join("\n");
    await api.writeTextFile(path, header + rows).catch((e) => setError(String(e)));
  };

  return (
    <div className="flex-1 overflow-y-auto bg-slate-50 p-6 md:p-10">
      <div className="max-w-4xl mx-auto space-y-5">
        <button
          type="button"
          onClick={() => onNav("timeline")}
          className="flex items-center gap-1.5 text-sm text-slate-500 hover:text-slate-700 cursor-pointer"
        >
          <ArrowLeft className="w-4 h-4" /> 返回时间线
        </button>

        <div className="flex items-center gap-3">
          <div className="w-11 h-11 rounded-xl bg-amber-50 flex items-center justify-center text-amber-600 border border-amber-100">
            <ShieldAlert className="w-6 h-6" />
          </div>
          <div>
            <h1 className="text-2xl font-bold text-slate-900">审计追踪</h1>
            <span className="text-[11px] font-mono text-slate-400 tracking-widest uppercase">
              Audit Trail · Hidden
            </span>
          </div>
        </div>

        <div className="rounded-xl px-4 py-3 text-sm bg-amber-50 text-amber-800 leading-relaxed">
          审计追踪:所有导入/导出/分享均由不可变事件日志记录(含内容哈希 sha256),可核验、防篡改。
        </div>

        {error && (
          <div className="rounded-xl px-4 py-2.5 text-sm bg-rose-50 text-rose-700">{error}</div>
        )}

        <div className="flex justify-end">
          <button
            type="button"
            onClick={exportManifest}
            disabled={entries.length === 0}
            className="flex items-center gap-2 text-sm font-medium text-slate-700 bg-white border border-slate-200 hover:bg-slate-50 disabled:opacity-50 rounded-xl px-4 py-2 transition-colors cursor-pointer"
          >
            <FileDown className="w-4 h-4" /> 导出审计清单
          </button>
        </div>

        <div className="bg-white rounded-2xl border border-slate-200 shadow-sm overflow-hidden">
          <table className="w-full text-sm">
            <thead className="bg-slate-50 text-slate-500 text-xs uppercase tracking-wide">
              <tr>
                <th className="text-left font-medium px-4 py-2.5">时间</th>
                <th className="text-left font-medium px-4 py-2.5">动作</th>
                <th className="text-left font-medium px-4 py-2.5">文件/详情</th>
                <th className="text-left font-medium px-4 py-2.5">哈希</th>
                <th className="text-left font-medium px-4 py-2.5">设备</th>
              </tr>
            </thead>
            <tbody>
              {entries.map((e) => (
                <tr key={e.seq} className="border-t border-slate-100">
                  <td className="px-4 py-2.5 text-xs font-mono text-slate-500 whitespace-nowrap">
                    {fmtTs(e.timestamp)}
                  </td>
                  <td className="px-4 py-2.5">
                    <span
                      className={`text-xs font-medium px-2 py-0.5 rounded-full ${
                        ACTION_BADGE[e.action] ?? "bg-slate-100 text-slate-600"
                      }`}
                    >
                      {e.action}
                    </span>
                  </td>
                  <td className="px-4 py-2.5 text-slate-700 max-w-xs truncate" title={e.detail}>
                    {e.detail}
                  </td>
                  <td className="px-4 py-2.5">
                    {e.sha256 ? (
                      <button
                        type="button"
                        onClick={() => copyHash(e.seq, e.sha256 as string)}
                        title={e.sha256}
                        className="flex items-center gap-1.5 text-xs font-mono text-slate-500 hover:text-blue-600 cursor-pointer"
                      >
                        {copiedSeq === e.seq ? (
                          <Check className="w-3 h-3" />
                        ) : (
                          <Copy className="w-3 h-3" />
                        )}
                        {shortHash(e.sha256)}
                      </button>
                    ) : (
                      <span className="text-xs text-slate-300">—</span>
                    )}
                  </td>
                  <td className="px-4 py-2.5 text-xs font-mono text-slate-400 whitespace-nowrap">
                    {e.device_id.slice(0, 8)}
                  </td>
                </tr>
              ))}
              {entries.length === 0 && !error && (
                <tr>
                  <td colSpan={5} className="px-4 py-8 text-center text-sm text-slate-400">
                    暂无记录
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </div>
    </div>
  );
}
