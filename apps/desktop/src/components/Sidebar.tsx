import { useState } from "react";
import {
  Activity,
  ShieldCheck,
  UploadCloud,
  Search,
  PanelLeftClose,
  PanelLeftOpen,
} from "lucide-react";

const NAV = [
  { id: "timeline", label: "生命时间线", sub: "Medical Lifeline", icon: Activity },
  { id: "search", label: "搜索", sub: "Search", icon: Search },
  { id: "import", label: "导入病历", sub: "Import Records", icon: UploadCloud },
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
          <div className="w-10 h-10 rounded-xl bg-blue-50 flex items-center justify-center text-blue-600 border border-blue-100 shrink-0">
            <ShieldCheck className="w-6 h-6" />
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
                  <div className="min-w-0">
                    <span
                      className={`text-sm font-medium block ${
                        active ? "text-blue-900" : "text-slate-700"
                      }`}
                    >
                      {item.label}
                    </span>
                    <span className="text-[10px] font-mono text-slate-400 block">{item.sub}</span>
                  </div>
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

      {/* Collapse toggle + footer */}
      <div className="p-2 border-t border-slate-200">
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
            <span>v0.1</span>
          </div>
        )}
      </div>
    </div>
  );
}
