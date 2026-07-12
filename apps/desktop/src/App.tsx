import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import ErrorBoundary from "./components/ErrorBoundary";
import Sidebar from "./components/Sidebar";
import PatientBanner from "./components/PatientBanner";
import Timeline from "./components/Timeline";
import DocumentView from "./components/DocumentView";
import ImportView from "./components/ImportView";
import SearchView from "./components/SearchView";
import AboutView from "./components/AboutView";
import SettingsView from "./components/SettingsView";
import AuditView from "./components/AuditView";
import { api } from "./api";
import type { TimelineGroup, DocumentDetail } from "./types";
import "./App.css";

export default function App() {
  const [groups, setGroups] = useState<TimelineGroup[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [detail, setDetail] = useState<DocumentDetail | null>(null);
  const [tab, setTab] = useState<string>("timeline");
  const [reloadKey, setReloadKey] = useState(0);
  const [loadingDemo, setLoadingDemo] = useState(false);
  const [demoError, setDemoError] = useState<string | null>(null);

  const loadTimeline = () =>
    api.listTimelineGrouped().then(setGroups).catch((e) => setError(String(e)));

  useEffect(() => {
    loadTimeline();
  }, []);

  const totalDocs = groups.reduce(
    (n, g) => n + (g.group_type === "encounter" ? g.encounter.doc_count : 1),
    0,
  );

  const openDoc = (id: number) => {
    setError(null);
    api.getDocument(id).then(setDetail).catch((e) => setError(String(e)));
  };

  const nav = (id: string) => {
    setDetail(null);
    setTab(id);
  };

  const afterImport = () => {
    loadTimeline();
    setReloadKey((k) => k + 1); // 让病人 banner 重新归纳
  };

  // 空时间线状态下的「加载示例数据」入口(见 Timeline.tsx),与 ImportView 里的
  // 同名按钮走同一条命令,方便测试者不必先切到「导入」页就能一键试用。
  const loadDemo = () => {
    setLoadingDemo(true);
    setDemoError(null);
    api
      .loadDemoData()
      .then(() => afterImport())
      .catch((e) => setDemoError(String(e)))
      .finally(() => setLoadingDemo(false));
  };

  // 收件箱(Watch Folder)自动导入完成后,后端会发出 vault-changed;这里刷新时间线 + banner
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen("vault-changed", () => {
      afterImport();
    }).then((f) => {
      unlisten = f;
    });
    return () => {
      if (unlisten) unlisten();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div className="w-screen h-screen flex bg-slate-50 overflow-hidden text-slate-800">
      <Sidebar activeTab={tab} onNav={nav} count={totalDocs} />
      <div className="flex-1 flex flex-col h-full overflow-hidden">
        <PatientBanner reloadKey={reloadKey} />
        {error && (
          <div className="px-6 py-3 text-rose-700 text-sm bg-rose-50 border-b border-rose-100">
            加载失败:{error}
          </div>
        )}
        {/* 底层视图常驻(不卸载),详情以覆盖层显示 → 返回时保留展开态/搜索词/滚动位置 */}
        <div className="flex-1 relative overflow-hidden flex flex-col">
          <ErrorBoundary>
            {tab === "import" ? (
              <ImportView onImported={afterImport} />
            ) : tab === "search" ? (
              <SearchView onSelect={openDoc} />
            ) : tab === "about" ? (
              <AboutView onNav={nav} />
            ) : tab === "settings" ? (
              <SettingsView onNav={nav} onReset={afterImport} />
            ) : tab === "audit" ? (
              <AuditView onNav={nav} />
            ) : (
              <Timeline
                groups={groups}
                onSelect={openDoc}
                onLoadDemo={loadDemo}
                loadingDemo={loadingDemo}
                demoError={demoError}
              />
            )}
            {detail && (
              <div className="absolute inset-0 z-10 bg-slate-50 flex flex-col">
                <DocumentView detail={detail} onBack={() => setDetail(null)} />
              </div>
            )}
          </ErrorBoundary>
        </div>
      </div>
    </div>
  );
}
