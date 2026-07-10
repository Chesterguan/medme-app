import { useEffect, useState } from "react";
import {
  Settings as SettingsIcon,
  FolderOpen,
  FolderSync,
  Inbox,
  UploadCloud,
  Info,
  CloudCog,
  Lock,
} from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../api";

export default function SettingsView({ onNav }: { onNav: (id: string) => void }) {
  const [vaultPath, setVaultPath] = useState<string | null>(null);
  const [inboxPath, setInboxPath] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [relocating, setRelocating] = useState(false);

  useEffect(() => {
    api.getVaultPath().then(setVaultPath).catch((e) => setError(String(e)));
    api.getInboxPath().then(setInboxPath).catch((e) => setError(String(e)));
  }, []);

  // 更换保险箱位置:从原生「选择文件夹」对话框选一个目录(选云同步文件夹即可多设备同步),
  // 后端会把现有病历整体搬过去(或在共享文件夹里与另一台设备合并),再返回新路径。
  async function changeLocation() {
    setError(null);
    const picked = await open({
      directory: true,
      multiple: false,
      title: "选择数据保险箱新位置",
    }).catch((e) => {
      setError(String(e));
      return null;
    });
    if (!picked || Array.isArray(picked)) return;
    setRelocating(true);
    try {
      const newPath = await api.setVaultPath(picked);
      setVaultPath(newPath);
    } catch (e) {
      setError(String(e));
    } finally {
      setRelocating(false);
    }
  }

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
              {relocating ? "正在搬迁病历…" : vaultPath ?? "加载中…"}
            </span>
            <div className="shrink-0 flex items-center gap-2">
              <button
                type="button"
                disabled={relocating}
                onClick={changeLocation}
                className="flex items-center gap-1.5 text-xs font-medium text-blue-700 bg-blue-50 hover:bg-blue-100 disabled:opacity-50 rounded-lg px-3 py-1.5 transition-colors cursor-pointer"
              >
                <FolderSync className="w-3.5 h-3.5" /> 更换位置…
              </button>
              <button
                type="button"
                disabled={!vaultPath || relocating}
                onClick={() =>
                  vaultPath && api.openPath(vaultPath).catch((e) => setError(String(e)))
                }
                className="flex items-center gap-1.5 text-xs font-medium text-blue-700 bg-blue-50 hover:bg-blue-100 disabled:opacity-50 rounded-lg px-3 py-1.5 transition-colors cursor-pointer"
              >
                <FolderOpen className="w-3.5 h-3.5" /> 打开文件夹
              </button>
            </div>
          </div>
          <div className="mt-3 flex items-start gap-2 text-sm text-slate-500 leading-relaxed">
            <CloudCog className="w-4 h-4 text-slate-400 shrink-0 mt-0.5" />
            <span>
              把这个文件夹放进云同步目录,多设备就自动同步(去中心化,无需服务器):
              <b className="text-slate-700">设备全是苹果 → iCloud 云盘;有安卓 / Windows → 坚果云</b>。
            </span>
          </div>
          <div className="mt-2 flex items-start gap-2 text-xs text-slate-400 leading-relaxed">
            <FolderSync className="w-3.5 h-3.5 shrink-0 mt-0.5" />
            <span>
              选一个云同步文件夹(iCloud 云盘 / 坚果云)即可多设备同步;换位置会把现有病历一起搬过去。
            </span>
          </div>
        </div>

        {/* 数据安全:引导用系统级端到端加密(零口令、老人无感);app 层口令加密留后续版本 */}
        <div className="bg-white rounded-2xl border border-slate-200 p-5 shadow-sm">
          <div className="flex items-center gap-2 text-slate-800 font-medium mb-2">
            <Lock className="w-5 h-5 text-blue-500" /> 数据安全 · 加密
          </div>
          <div className="text-sm text-slate-500 leading-relaxed mb-3">
            病历默认存在本机。想要更强保护(尤其把保险箱放到云盘同步时),开启系统级端到端加密即可,
            <b className="text-slate-700">无需在本 app 记任何口令</b>,一次设置、家人可代劳:
          </div>
          <ol className="space-y-2.5 text-sm text-slate-600 leading-relaxed list-none">
            <li className="flex gap-2.5">
              <span className="shrink-0 w-5 h-5 rounded-full bg-blue-100 text-blue-700 text-xs font-bold flex items-center justify-center">
                1
              </span>
              <span>
                开启 <b className="text-slate-800">Mac FileVault</b>(全盘加密):系统设置 › 隐私与安全性 ›
                FileVault › 打开。本机数据即加密存储。
              </span>
            </li>
            <li className="flex gap-2.5">
              <span className="shrink-0 w-5 h-5 rounded-full bg-blue-100 text-blue-700 text-xs font-bold flex items-center justify-center">
                2
              </span>
              <span>
                开启 <b className="text-slate-800">iCloud 高级数据保护</b>(端到端,苹果也读不了):系统设置 ›
                [你的名字] › iCloud › 高级数据保护 › 打开。云端同步的数据即端到端加密。
              </span>
            </li>
          </ol>
          <div className="mt-3 text-xs text-slate-400 leading-relaxed">
            两者一起 = 本机 + 云端都端到端加密。app 内置的口令加密(适配 iCloud 之外的第三方云)将在后续版本提供。
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

        <div className="text-xs font-mono text-slate-400 text-center">版本 v1.0</div>
      </div>
    </div>
  );
}
