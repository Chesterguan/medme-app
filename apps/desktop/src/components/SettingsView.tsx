import { useEffect, useState } from "react";
import { Settings as SettingsIcon, FolderOpen, Inbox, UploadCloud, Info, CloudCog } from "lucide-react";
import { api } from "../api";

export default function SettingsView({ onNav }: { onNav: (id: string) => void }) {
  const [vaultPath, setVaultPath] = useState<string | null>(null);
  const [inboxPath, setInboxPath] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.getVaultPath().then(setVaultPath).catch((e) => setError(String(e)));
    api.getInboxPath().then(setInboxPath).catch((e) => setError(String(e)));
  }, []);

  return (
    <div className="flex-1 overflow-y-auto bg-slate-50 p-6 md:p-10">
      <div className="max-w-2xl mx-auto space-y-5">
        <div className="flex items-center gap-3">
          <div className="w-11 h-11 rounded-xl bg-blue-50 flex items-center justify-center text-blue-600 border border-blue-100">
            <SettingsIcon className="w-6 h-6" />
          </div>
          <div>
            <h1 className="text-2xl font-bold text-slate-900">设置</h1>
            <span className="text-[11px] font-mono text-slate-400 tracking-widest uppercase">
              MedMe 医我
            </span>
          </div>
        </div>

        {error && (
          <div className="rounded-xl px-4 py-2.5 text-sm bg-rose-50 text-rose-700">{error}</div>
        )}

        {/* 数据保险箱位置 */}
        <div className="bg-white rounded-2xl border border-slate-200 p-5 shadow-sm">
          <div className="flex items-center gap-2 text-slate-800 font-medium mb-2">
            <FolderOpen className="w-5 h-5 text-blue-500" /> 数据保险箱位置
          </div>
          <div className="flex items-center justify-between gap-3 bg-slate-50 border border-slate-200 rounded-xl px-4 py-2.5">
            <span className="text-xs font-mono text-slate-600 truncate">
              {vaultPath ?? "加载中…"}
            </span>
            <button
              type="button"
              disabled={!vaultPath}
              onClick={() =>
                vaultPath && api.openPath(vaultPath).catch((e) => setError(String(e)))
              }
              className="shrink-0 flex items-center gap-1.5 text-xs font-medium text-blue-700 bg-blue-50 hover:bg-blue-100 disabled:opacity-50 rounded-lg px-3 py-1.5 transition-colors cursor-pointer"
            >
              <FolderOpen className="w-3.5 h-3.5" /> 打开文件夹
            </button>
          </div>
          <div className="mt-3 flex items-start gap-2 text-sm text-slate-500 leading-relaxed">
            <CloudCog className="w-4 h-4 text-slate-400 shrink-0 mt-0.5" />
            <span>
              把这个文件夹放到 iCloud / 坚果云 等云同步目录,即可多设备同步(去中心化,无需服务器)。
            </span>
          </div>
        </div>

        {/* 自动收件箱 */}
        <div className="bg-white rounded-2xl border border-slate-200 p-5 shadow-sm">
          <div className="flex items-center gap-2 text-slate-800 font-medium mb-2">
            <Inbox className="w-5 h-5 text-blue-500" /> 自动收件箱
          </div>
          <div className="flex items-center justify-between gap-3 bg-slate-50 border border-slate-200 rounded-xl px-4 py-2.5">
            <span className="text-xs font-mono text-slate-600 truncate">
              {inboxPath ?? "加载中…"}
            </span>
            <button
              type="button"
              onClick={() => api.openInbox().catch((e) => setError(String(e)))}
              className="shrink-0 flex items-center gap-1.5 text-xs font-medium text-blue-700 bg-blue-50 hover:bg-blue-100 rounded-lg px-3 py-1.5 transition-colors cursor-pointer"
            >
              <FolderOpen className="w-3.5 h-3.5" /> 打开
            </button>
          </div>
        </div>

        {/* 导入 / 导出 / 加密分享:不重复放控件,指向对应页面 */}
        <div className="bg-white rounded-2xl border border-slate-200 p-5 shadow-sm">
          <div className="flex items-center gap-2 text-slate-800 font-medium mb-2">
            <UploadCloud className="w-5 h-5 text-blue-500" /> 导入 · 导出 · 加密分享
          </div>
          <div className="flex items-center justify-between gap-3">
            <span className="text-sm text-slate-500">在「导入·导出」页操作。</span>
            <button
              type="button"
              onClick={() => onNav("import")}
              className="shrink-0 text-xs font-medium text-blue-700 bg-blue-50 hover:bg-blue-100 rounded-lg px-3 py-1.5 transition-colors cursor-pointer"
            >
              前往
            </button>
          </div>
        </div>

        {/* 关于 · 声明 */}
        <div className="bg-white rounded-2xl border border-slate-200 p-5 shadow-sm">
          <div className="flex items-center justify-between gap-3">
            <div className="flex items-center gap-2 text-slate-800 font-medium">
              <Info className="w-5 h-5 text-blue-500" /> 关于 · 声明
            </div>
            <button
              type="button"
              onClick={() => onNav("about")}
              className="shrink-0 text-xs font-medium text-blue-700 bg-blue-50 hover:bg-blue-100 rounded-lg px-3 py-1.5 transition-colors cursor-pointer"
            >
              查看
            </button>
          </div>
        </div>

        <div className="text-xs font-mono text-slate-400 text-center">版本 v0.1</div>
      </div>
    </div>
  );
}
