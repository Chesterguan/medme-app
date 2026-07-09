import { useState } from "react";
import {
  Activity,
  UploadCloud,
  Search,
  PanelLeftClose,
  PanelLeftOpen,
  Info,
  Settings,
} from "lucide-react";

const NAV = [
  { id: "timeline", label: "生命时间线", icon: Activity },
  { id: "search", label: "搜索", icon: Search },
  { id: "import", label: "导入 · 导出", icon: UploadCloud },
];

export default function Sidebar({
  activeTab,
  onNav,
  count,
}: {
  activeTab: string;
  onNav: (id: string) => void;
  count: number;
}) {
  const [collapsed, setCollapsed] = useState(false);

  return (
    <div
      className={`${
        collapsed ? "w-16" : "w-60"
      } bg-white border-r border-slate-200 flex flex-col h-screen text-slate-600 select-none shrink-0 transition-[width] duration-200`}
    >
      {/* Brand */}
      <div className={`${collapsed ? "px-2 py-4" : "p-5"} border-b border-slate-200`}>
        <div className={`flex items-center ${collapsed ? "justify-center" : "gap-3"}`}>
          <div
            className="w-10 h-10 rounded-xl flex items-center justify-center text-white font-black shrink-0"
            style={{
              background: "linear-gradient(148deg,#2bc0d2 0%,#1789c1 52%,#1560a8 100%)",
              fontFamily: '"PingFang SC","Hiragino Sans GB",sans-serif',
              fontSize: "22px",
              lineHeight: 1,
            }}
          >
            <span style={{ transform: "translateY(-3%)" }}>医</span>
          </div>
          {!collapsed && (
            <div className="min-w-0">
              <div className="flex items-center gap-1.5">
                <span className="font-bold text-lg text-blue-600 tracking-tight">MedMe</span>
                <span className="font-bold text-lg text-slate-950">医我</span>
              </div>
              <span className="text-[10px] font-mono text-slate-400 tracking-widest uppercase block mt-0.5">
                Health Vault
              </span>
              <span className="text-[10px] text-slate-400 block mt-0.5">个人医疗数据保险箱</span>
            </div>
          )}
        </div>
      </div>

      {/* Nav */}
      <nav className="flex-1 p-2 space-y-1">
        {NAV.map((item) => {
          const Icon = item.icon;
          const active = activeTab === item.id;
          return (
            <button
              key={item.id}
              onClick={() => onNav(item.id)}
              title={collapsed ? item.label : undefined}
              className={`w-full flex items-center ${
                collapsed ? "justify-center p-2.5" : "justify-between p-3"
              } rounded-xl transition-all cursor-pointer text-left ${
                active
                  ? "bg-blue-50 text-blue-700 border border-blue-100/40"
                  : "border border-transparent hover:bg-slate-50"
              }`}
            >
              <div className={`flex items-center ${collapsed ? "" : "gap-3"} min-w-0`}>
                <Icon
                  className={`w-5 h-5 shrink-0 ${active ? "text-blue-600" : "text-slate-400"}`}
                />
                {!collapsed && (
                  <span
                    className={`text-sm font-medium ${
                      active ? "text-blue-900" : "text-slate-700"
                    }`}
                  >
                    {item.label}
                  </span>
                )}
              </div>
              {!collapsed && item.id === "timeline" && (
                <span
                  className={`px-2 py-0.5 rounded-full text-[10px] font-bold font-mono ${
                    active ? "bg-blue-600 text-white" : "bg-slate-100 text-slate-600"
                  }`}
                >
                  {count}
                </span>
              )}
            </button>
          );
        })}
      </nav>

      {/* Footer */}
      <div className="p-2 border-t border-slate-200">
        <div className={`flex ${collapsed ? "flex-col gap-1" : "gap-1"}`}>
          <button
            onClick={() => onNav("about")}
            title={collapsed ? "关于 · 声明" : undefined}
            className={`flex-1 flex items-center ${
              collapsed ? "justify-center" : "gap-2"
            } p-2.5 rounded-xl cursor-pointer transition-colors ${
              activeTab === "about"
                ? "bg-blue-50 text-blue-700"
                : "text-slate-400 hover:bg-slate-50 hover:text-slate-600"
            }`}
          >
            <Info className="w-5 h-5 shrink-0" />
            {!collapsed && <span className="text-xs font-medium">关于 · 声明</span>}
          </button>
          <button
            onClick={() => onNav("settings")}
            title="设置"
            className={`flex items-center justify-center p-2.5 rounded-xl cursor-pointer transition-colors shrink-0 ${
              activeTab === "settings"
                ? "bg-blue-50 text-blue-700"
                : "text-slate-400 hover:bg-slate-50 hover:text-slate-600"
            }`}
          >
            <Settings className="w-5 h-5 shrink-0" />
          </button>
        </div>
        <button
          onClick={() => setCollapsed((c) => !c)}
          title={collapsed ? "展开" : "收起"}
          className={`w-full flex items-center ${
            collapsed ? "justify-center" : "gap-2"
          } p-2.5 rounded-xl text-slate-400 hover:bg-slate-50 hover:text-slate-600 cursor-pointer transition-colors`}
        >
          {collapsed ? (
            <PanelLeftOpen className="w-5 h-5" />
          ) : (
            <>
              <PanelLeftClose className="w-5 h-5" />
              <span className="text-xs font-mono">收起侧栏</span>
            </>
          )}
        </button>
        {!collapsed && (
          <div className="px-2 pt-2 text-[10px] font-mono text-slate-400 flex justify-between">
            <span>© MedMe 2026</span>
            <span>v1.0</span>
          </div>
        )}
      </div>
    </div>
  );
}
