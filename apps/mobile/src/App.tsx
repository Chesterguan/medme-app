import { useCallback, useEffect, useRef, useState } from "react";
import type { ChangeEvent, ReactNode } from "react";
import "./App.css";
import markUrl from "./assets/medme-mark.svg";
import { api } from "./api";
import type {
  TimelineGroup,
  ImportOutcome,
  ShareResult,
  PatientProfile,
  DocumentDetail,
  IcloudStatus,
} from "./types";
import {
  DocTypeIcon,
  EncounterIcon,
  FileTextIcon,
  FolderIcon,
  DownloadIcon,
  TrashIcon,
  AlertTriangleIcon,
  CheckCircleIcon,
  ArrowLeftIcon,
  LinkIcon,
  ShareIcon,
  CopyIcon,
  EyeIcon,
  ShieldIcon,
} from "./icons";

// doc_type / encounter kind → 中文标签(见 core-model types.rs)
const DOC_LABEL: Record<string, string> = {
  lab_report: "化验",
  imaging_report: "影像",
  discharge_summary: "出院小结",
  prescription: "处方",
  clinical_note: "病历",
  pathology: "病理",
  surgery: "手术",
  other: "其他",
  unknown: "待归类",
};
const KIND_LABEL: Record<string, string> = {
  inpatient: "住院",
  outpatient: "门诊",
  emergency: "急诊",
  exam: "检查",
};

function fmtDate(iso: string | null): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(
    d.getDate(),
  ).padStart(2, "0")}`;
}

function groupTitle(g: TimelineGroup): string {
  if (g.group_type === "encounter") {
    const e = g.encounter;
    const kind = KIND_LABEL[e.kind] ?? e.kind;
    return e.provider ? `${kind} · ${e.provider}` : kind;
  }
  return g.doc.title ?? DOC_LABEL[g.doc.doc_type] ?? "记录";
}

function groupDate(g: TimelineGroup): string {
  return fmtDate(g.group_type === "encounter" ? g.encounter.start_date : g.doc.doc_date);
}

function groupDesc(g: TimelineGroup): string {
  if (g.group_type === "encounter") {
    const kinds = new Set(g.docs.map((d) => DOC_LABEL[d.doc_type] ?? d.doc_type));
    const parts = [`${g.encounter.doc_count} 份记录`, ...Array.from(kinds).slice(0, 3)];
    if (g.encounter.transferred) parts.push("转院");
    return parts.join(" · ");
  }
  return DOC_LABEL[g.doc.doc_type] ?? g.doc.doc_type;
}

type Tab = "capture" | "archive" | "settings";

// iCloud 同步只在 iOS(苹果设备)上可用 —— 据此在设置里显示/隐藏开关。
// WKWebView 的 UA 含 iPhone/iPad/iPod;Android WebView 含 Android,故不显示。
const IS_IOS =
  typeof navigator !== "undefined" && /iP(hone|ad|od)/i.test(navigator.userAgent);

export default function App() {
  const [tab, setTab] = useState<Tab>("capture");
  const [groups, setGroups] = useState<TimelineGroup[]>([]);
  const [profile, setProfile] = useState<PatientProfile | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [lastImport, setLastImport] = useState<ImportOutcome | null>(null);
  const [share, setShare] = useState<ShareResult | null>(null);
  const [confirmReset, setConfirmReset] = useState(false);
  const [confirmDisableIcloud, setConfirmDisableIcloud] = useState(false);
  const [version, setVersion] = useState("");
  // iCloud 同步状态(仅 iOS)。null = 尚未查询 / 不适用。
  const [icloud, setIcloud] = useState<IcloudStatus | null>(null);
  // 点开的文档 id —— 非空时全屏展示文档详情(见 DetailScreen)。
  const [detailId, setDetailId] = useState<number | null>(null);
  // 就诊组在档案里点开时展开其子文档(每份可再点开详情)。
  const [expanded, setExpanded] = useState<Set<number>>(new Set());
  // 采集来源选择弹层(拍照 / 从相册选)。
  const [chooser, setChooser] = useState(false);
  // 两个隐藏 file input:相机(capture=environment 直接调起后置相机)与相册(无 capture)。
  const cameraInputRef = useRef<HTMLInputElement>(null);
  const libraryInputRef = useRef<HTMLInputElement>(null);

  const refresh = useCallback(async () => {
    try {
      const [g, p] = await Promise.all([api.loadArchive(), api.getPatientProfile()]);
      setGroups(g);
      setProfile(p);
    } catch (e) {
      console.error("refresh failed", e);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  // 版本号取自 tauri.conf.json(与 App 包一致)。延迟加载,失败也不影响首屏。
  useEffect(() => {
    import("@tauri-apps/api/app")
      .then(({ getVersion }) => getVersion())
      .then(setVersion)
      .catch(() => {});
  }, []);

  // iCloud 同步状态:仅 iOS 查询(Android/桌面无 iCloud,开关不显示)。
  useEffect(() => {
    if (!IS_IOS) return;
    api.icloudStatus().then(setIcloud).catch(() => {});
  }, []);

  // 开启 iCloud 同步:把保险箱真相迁入 iCloud 容器(数据库仍留本地并重建)。
  // 一次性开启;开启后各苹果设备自动同步。失败(未登录 iCloud)诚实提示,不改动本地库。
  const enableIcloud = useCallback(async () => {
    if (icloud?.enabled) return; // 已开启,幂等
    try {
      setBusy("正在开启 iCloud 同步…");
      await api.enableIcloudSync();
      setIcloud({ available: true, enabled: true });
      await refresh();
    } catch (e) {
      alert(`开启 iCloud 同步失败:${e}`);
    } finally {
      setBusy(null);
    }
  }, [icloud, refresh]);

  // 关闭 iCloud 同步:把真相复制回本机(iCloud 里的副本保留),本地成为主副本;
  // 之后本设备不再自动同步。用 disable_icloud_sync(copy_to,不删源)。
  const disableIcloud = useCallback(async () => {
    try {
      setBusy("正在关闭 iCloud 同步…");
      await api.disableIcloudSync();
      setIcloud({ available: true, enabled: false });
      setConfirmDisableIcloud(false);
      await refresh();
    } catch (e) {
      alert(`关闭 iCloud 同步失败:${e}`);
    } finally {
      setBusy(null);
    }
  }, [refresh]);

  // 采集:iOS WKWebView 里最可靠的相机路径 = 隐藏的 HTML file input。
  //  - `accept="image/*" capture="environment"` → 直接调起后置相机拍照;
  //  - `accept="image/*"`(无 capture)→ 相册选择。
  // 选中后拿到的是 File 对象(WKWebView 里没有沙盒文件系统路径可给 Rust),
  // 于是读出字节交给 `ingest_bytes`:后端写临时文件再走同一套 pipeline ingest。
  // 这替代了原先的 tauri-plugin-dialog open()(那是「文件」文档选择器,没有相机,
  // 正是「只能选图、无法拍照」的根因)。
  const onPicked = useCallback(
    async (e: ChangeEvent<HTMLInputElement>) => {
      const input = e.currentTarget;
      const file = input.files?.[0];
      input.value = ""; // 允许连续选同一张
      setChooser(false);
      if (!file) return; // 用户取消
      setShare(null);
      try {
        setBusy("识别中…");
        const buf = await file.arrayBuffer();
        const data = Array.from(new Uint8Array(buf));
        const outcome = await api.ingestBytes(file.name || "capture.jpg", data);
        setLastImport(outcome);
        await refresh();
      } catch (err) {
        console.error("capture failed", err);
        alert(`采集失败:${err}`);
      } finally {
        setBusy(null);
      }
    },
    [refresh],
  );

  const loadDemo = useCallback(async () => {
    setShare(null);
    try {
      setBusy("正在载入示例数据…");
      const n = await api.loadDemoData();
      setLastImport({ name: `示例数据 ${n} 份`, source_file_id: 0, status: "new", doc_type: null });
      await refresh();
      setTab("archive");
    } catch (e) {
      alert(`载入示例失败:${e}`);
    } finally {
      setBusy(null);
    }
  }, [refresh]);

  // 清空保险箱 · 重置:让示例数据可逆(载入 → 试用 → 清空 → 从头开始)。
  const resetVault = useCallback(async () => {
    setShare(null);
    setLastImport(null);
    try {
      setBusy("正在清空保险箱…");
      await api.resetVault();
      await refresh();
      setConfirmReset(false);
    } catch (e) {
      alert(`清空失败:${e}`);
    } finally {
      setBusy(null);
    }
  }, [refresh]);

  const doShare = useCallback(async () => {
    setLastImport(null);
    try {
      setBusy("正在生成端到端加密分享…");
      const r = await api.createShare(5);
      setShare(r);
    } catch (e) {
      alert(`生成分享失败:${e}`);
    } finally {
      setBusy(null);
    }
  }, []);

  // 项目主页:iOS WKWebView 里 <a target> 不会拉起系统浏览器,必须走
  // tauri-plugin-opener 的 openUrl()(会用系统默认浏览器 Safari 打开)。
  const openHomepage = useCallback(async () => {
    try {
      const { openUrl } = await import("@tauri-apps/plugin-opener");
      await openUrl("https://chesterguan.github.io/medme/");
    } catch (e) {
      alert(`打开链接失败:${e}`);
    }
  }, []);

  // 点开一组:文档组 → 直接看详情;就诊组 → 展开/收起其子文档。
  const openGroup = useCallback((g: TimelineGroup, idx: number) => {
    if (g.group_type === "document") {
      setDetailId(g.doc.id);
    } else {
      setExpanded((prev) => {
        const next = new Set(prev);
        if (next.has(idx)) next.delete(idx);
        else next.add(idx);
        return next;
      });
    }
  }, []);

  const initial = profile?.name?.[0] ?? "我";
  const recent = groups.slice(0, 4);

  // 文档详情覆盖全屏(带返回)——优先于 tab 内容渲染。
  if (detailId != null) {
    return <DetailScreen id={detailId} onBack={() => setDetailId(null)} />;
  }

  return (
    <div className="app">
      <div className="appbar">
        <div className="brand">
          <img className="logo" src={markUrl} alt="医我" />
          医我
        </div>
        <div className="who">{initial}</div>
      </div>

      {tab === "capture" ? (
        <div className="body">
          <button className="shoot" onClick={() => setChooser(true)} disabled={!!busy}>
            <div className="cam">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <path d="M14.5 4h-5L7 7H4a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V9a2 2 0 0 0-2-2h-3l-2.5-3z" />
                <circle cx="12" cy="13" r="3.2" />
              </svg>
            </div>
            <b>拍照存档</b>
            <span>病历 · 化验单 · 报告,拍下或选图就存</span>
          </button>

          <div className="sect">最近导入</div>
          {recent.length === 0 ? (
            <div className="card">
              <div className="ic"><FileTextIcon /></div>
              <div className="tx">
                <b>还没有记录</b>
                <span>点上方拍照,或载入示例数据试试</span>
              </div>
            </div>
          ) : (
            recent.map((g, i) => (
              <button
                className="card tap"
                key={i}
                onClick={() =>
                  g.group_type === "document" ? setDetailId(g.doc.id) : setTab("archive")
                }
              >
                <div className={`ic t-${g.group_type === "document" ? g.doc.doc_type : "enc"}`}>
                  {g.group_type === "encounter" ? (
                    <EncounterIcon kind={g.encounter.kind} />
                  ) : (
                    <DocTypeIcon type={g.doc.doc_type} />
                  )}
                </div>
                <div className="tx">
                  <b>{groupTitle(g)}</b>
                  <span>{groupDesc(g)}</span>
                </div>
                <span className="meta">{groupDate(g)}</span>
              </button>
            ))
          )}

          <button className="btn ghost" onClick={loadDemo} disabled={!!busy}>
            载入示例数据(张建国)
          </button>
          <button className="btn primary" onClick={doShare} disabled={!!busy || (profile?.record_count ?? 0) === 0}>
            加密分享给医生
          </button>

          <div className="synced">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round">
              <path d="M20 6L9 17l-5-5" />
            </svg>
            数据保存在本机保险箱(iCloud 同步:v1.1)
          </div>
        </div>
      ) : tab === "archive" ? (
        <div className="body">
          <div className="phead">
            <div className="avatar">{initial}</div>
            <div>
              <div className="nm">{profile?.name ?? "我的健康档案"}</div>
              <div className="sub">
                {[profile?.gender, profile?.age].filter(Boolean).join(" · ")}
                {profile ? `${profile.gender || profile.age ? " · " : ""}${profile.record_count} 份记录` : ""}
              </div>
            </div>
          </div>

          {groups.length === 0 ? (
            <div className="empty">
              <div className="big"><FolderIcon /></div>
              健康档案还是空的
              <br />
              去「拍照」页采集或载入示例数据
            </div>
          ) : (
            <div className="tl">
              {groups.map((g, i) => (
                <div className="item" key={i}>
                  <span className={`node t-${g.group_type === "document" ? g.doc.doc_type : "enc"}`}>
                    {g.group_type === "encounter" ? (
                      <EncounterIcon kind={g.encounter.kind} />
                    ) : (
                      <DocTypeIcon type={g.doc.doc_type} />
                    )}
                  </span>
                  <div className="c">
                    <button className="crow" onClick={() => openGroup(g, i)}>
                      <div className="top">
                        <b>{groupTitle(g)}</b>
                        <span className="d">{groupDate(g)}</span>
                      </div>
                      <div className="desc">
                        {groupDesc(g)}
                        {g.group_type === "document" && g.doc.slice_count ? (
                          <>
                            {" · "}
                            <span className="kind">影像 {g.doc.slice_count} 张</span>
                          </>
                        ) : null}
                      </div>
                    </button>
                    {g.group_type === "encounter" && expanded.has(i) && (
                      <div className="subdocs">
                        {g.docs.map((d) => (
                          <button className="subdoc" key={d.id} onClick={() => setDetailId(d.id)}>
                            <span className={`sic t-${d.doc_type}`}>
                              <DocTypeIcon type={d.doc_type} />
                            </span>
                            <span className="sl">{d.title ?? DOC_LABEL[d.doc_type] ?? "记录"}</span>
                            <span className="sd">{fmtDate(d.doc_date)}</span>
                          </button>
                        ))}
                      </div>
                    )}
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>
      ) : (
        <div className="body">
          {/* 数据:载入 ↔ 清空 成对出现,让示例数据可逆 */}
          <div className="sect">数据</div>
          <div className="group">
            <button className="row" onClick={loadDemo} disabled={!!busy}>
              <span className="ri"><DownloadIcon /></span>
              <span className="rt">
                <b>载入示例数据(张建国)</b>
                <span>导入一份完整的示例病历,先试试看</span>
              </span>
              <span className="chev">›</span>
            </button>
            <button className="row danger" onClick={() => setConfirmReset(true)} disabled={!!busy}>
              <span className="ri"><TrashIcon /></span>
              <span className="rt">
                <b>清空保险箱 · 重置</b>
                <span>删除全部记录,回到初始空状态</span>
              </span>
              <span className="chev">›</span>
            </button>
          </div>

          {/* 同步:iOS 提供 iCloud 一键开启;其余诚实说明本地存 + 加密分享 */}
          <div className="sect">同步与备份</div>
          <div className="group">
            {IS_IOS && (
              <button
                className="row"
                onClick={() => (icloud?.enabled ? setConfirmDisableIcloud(true) : enableIcloud())}
                disabled={!!busy}
              >
                <span className="ri"><ShieldIcon /></span>
                <span className="rt">
                  <b>iCloud 同步{icloud?.enabled ? " · 已开启" : ""}</b>
                  <span>
                    {icloud?.enabled
                      ? "已在所有苹果设备间自动同步。点此关闭:数据搬回本机,iCloud 里的副本保留。"
                      : "开启后,你的病历在所有苹果设备间自动同步。"}
                  </span>
                </span>
                <span className="chev">›</span>
              </button>
            )}
            <div className="info">
              病历存在<b>这台手机</b>上。想给医生、或换一台设备看某份记录,
              用记录详情页里的<b>「加密分享」</b>——发一个加密文件 + 一串口令,不经过任何服务器。
            </div>
            <div className="info">
              <b>多设备自动同步:</b>iPhone / iPad 上用上面的<b>「iCloud 同步」</b>一键开启;
              <b>电脑端</b>把保险箱文件夹放进云盘也能同步——
              <b>全是苹果 → iCloud 云盘;有安卓 / Windows → 坚果云</b>。
            </div>
            <div className="info">
              <b>关于云端安全:</b>无论用 iCloud 还是别的云盘,资料在云端<b>没有额外加密</b>,安全取决于你系统本身的保护。
              建议开启 iPhone 的<b>「高级数据保护(ADP)」</b>:设置 › 点你的名字(Apple ID) › iCloud › 高级数据保护。
              开启后连苹果也读不了你的资料。
            </div>
          </div>

          {/* 关于 */}
          <div className="sect">关于</div>
          <div className="group">
            <div className="info">
              <div className="kv">
                版本号 <span>{version ? `v${version}` : "—"}</span>
              </div>
            </div>
            <button className="row" onClick={openHomepage}>
              <span className="ri"><LinkIcon /></span>
              <span className="rt">
                <b>项目主页</b>
                <span>chesterguan.github.io/medme</span>
              </span>
              <span className="chev">›</span>
            </button>
            <div className="info disc">
              医疗免责声明:MedMe 是个人医疗数据整理工具,非医疗器械,不提供任何诊断或医疗建议;一切以原始医疗文件为准,请咨询执业医师。
            </div>
          </div>
        </div>
      )}

      {/* 隐藏的采集输入:相机(capture=environment)与相册。放在渲染树里,由弹层触发 click。 */}
      <input
        ref={cameraInputRef}
        type="file"
        accept="image/*"
        capture="environment"
        hidden
        onChange={onPicked}
      />
      <input ref={libraryInputRef} type="file" accept="image/*" hidden onChange={onPicked} />

      {/* 采集来源选择:拍照(调起相机)/ 从相册选。 */}
      {chooser && (
        <div className="scrim" onClick={() => !busy && setChooser(false)}>
          <div className="dialog" onClick={(e) => e.stopPropagation()}>
            <h3>添加记录</h3>
            <p>拍摄病历、化验单、报告,或从相册选择已有照片存入健康档案。</p>
            <div className="acts">
              <button
                className="primary"
                onClick={() => {
                  setChooser(false);
                  cameraInputRef.current?.click();
                }}
                disabled={!!busy}
              >
                拍照
              </button>
              <button
                className="cancel"
                onClick={() => {
                  setChooser(false);
                  libraryInputRef.current?.click();
                }}
                disabled={!!busy}
              >
                从相册选
              </button>
            </div>
            <button className="full" onClick={() => setChooser(false)} disabled={!!busy}>
              取消
            </button>
          </div>
        </div>
      )}

      {/* 关闭 iCloud 确认:数据搬回本机,iCloud 里的副本保留 */}
      {confirmDisableIcloud && (
        <div className="scrim" onClick={() => !busy && setConfirmDisableIcloud(false)}>
          <div className="dialog" onClick={(e) => e.stopPropagation()}>
            <h3>关闭 iCloud 同步?</h3>
            <p>
              病历会复制回这台手机(本地成为主副本),iCloud 里的副本保留不动。
              之后这台设备不再自动同步。
            </p>
            <div className="acts">
              <button
                className="cancel"
                onClick={() => setConfirmDisableIcloud(false)}
                disabled={!!busy}
              >
                取消
              </button>
              <button className="confirm" onClick={disableIcloud} disabled={!!busy}>
                关闭同步
              </button>
            </div>
          </div>
        </div>
      )}
      {/* 清空确认:破坏性操作,二次确认 */}
      {confirmReset && (
        <div className="scrim" onClick={() => !busy && setConfirmReset(false)}>
          <div className="dialog" onClick={(e) => e.stopPropagation()}>
            <h3>清空保险箱?</h3>
            <p>确定清空全部记录?示例数据和已导入内容都会删除,此操作不可撤销。</p>
            <div className="acts">
              <button className="cancel" onClick={() => setConfirmReset(false)} disabled={!!busy}>
                取消
              </button>
              <button className="confirm" onClick={resetVault} disabled={!!busy}>
                清空
              </button>
            </div>
          </div>
        </div>
      )}

      {/* 识别确认(M1 简版):入库后弹条,展示自动归类结果。完整「确认/纠正」页 = M2。 */}
      {lastImport && (
        <div className="toast" onClick={() => setLastImport(null)}>
          <div className={`h ${lastImport.status === "failed" ? "warn" : "ok"}`}>
            {lastImport.status === "failed" ? <AlertTriangleIcon /> : <CheckCircleIcon />}
            {lastImport.status === "failed" ? "未能识别" : "已识别入库"}
          </div>
          <div>
            <b>{lastImport.name}</b>
            {lastImport.doc_type ? ` · 归类为「${DOC_LABEL[lastImport.doc_type] ?? lastImport.doc_type}」` : ""}
          </div>
          <div className="note">
            自动归类完成。<small>点此关闭 · 完整的「确认 / 纠正」页为 M2</small>
          </div>
        </div>
      )}

      {/* 加密分享结果:两步清晰动作 —— 分享文件(系统 sheet)+ 复制口令(另发)。 */}
      {share && <ShareModal share={share} onClose={() => setShare(null)} />}

      {busy && (
        <div className="toast">
          <div className="h">{busy}</div>
        </div>
      )}

      <div className="tabbar">
        <button className={`t ${tab === "capture" ? "on" : ""}`} onClick={() => setTab("capture")}>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
            <path d="M14.5 4h-5L7 7H4a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V9a2 2 0 0 0-2-2h-3l-2.5-3z" />
            <circle cx="12" cy="13" r="3" />
          </svg>
          拍照
        </button>
        <button className={`t ${tab === "archive" ? "on" : ""}`} onClick={() => setTab("archive")}>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
            <line x1="8" y1="6" x2="21" y2="6" />
            <line x1="8" y1="12" x2="21" y2="12" />
            <line x1="8" y1="18" x2="21" y2="18" />
            <line x1="3" y1="6" x2="3.01" y2="6" />
            <line x1="3" y1="12" x2="3.01" y2="12" />
            <line x1="3" y1="18" x2="3.01" y2="18" />
          </svg>
          档案
        </button>
        <button className={`t ${tab === "settings" ? "on" : ""}`} onClick={() => setTab("settings")}>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <circle cx="12" cy="12" r="3" />
            <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
          </svg>
          设置
        </button>
      </div>
    </div>
  );
}

// 加密分享结果:自包含加密 HTML 文件(数据+查看器内嵌,零服务器)。给用户两个
// 清晰动作 —— (1)分享文件:调起 iOS 系统「分享」sheet 把 .html 发给医生;
// (2)复制口令:口令必须「另发」(不同渠道)。医生打开文件输口令即可查看。
function ShareModal({ share, onClose }: { share: ShareResult; onClose: () => void }) {
  const [copied, setCopied] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const [sharing, setSharing] = useState(false);

  const filename = share.path.split("/").pop() || "medme-share.html";

  // 分享文件:优先 Web Share API(WKWebView 里会拉起 iOS 系统分享 sheet,可发
  // 微信/邮件/AirDrop);不支持时回退到 opener 打开文件(Quick Look 自带分享按钮),
  // 再不行就明确告知文件位置。口令绝不放进分享内容 —— 必须另发。
  const doShareFile = useCallback(async () => {
    setMsg(null);
    setSharing(true);
    try {
      const buf = await api.readShareBytes(share.path);
      const file = new File([buf], filename, { type: "text/html" });
      const nav = navigator as Navigator & {
        canShare?: (d?: unknown) => boolean;
        share?: (d: unknown) => Promise<void>;
      };
      if (nav.share) {
        try {
          if (!nav.canShare || nav.canShare({ files: [file] })) {
            await nav.share({ files: [file], title: "MedMe 加密病历" });
            return;
          }
        } catch (e) {
          if ((e as Error)?.name === "AbortError") return; // 用户在 sheet 里取消
          // 否则落到下面的回退
        }
      }
      // 回退:用系统默认程序打开文件(iOS 会用 Quick Look,内含「分享」按钮)。
      const { openPath } = await import("@tauri-apps/plugin-opener");
      await openPath(share.path);
    } catch (e) {
      if ((e as Error)?.name === "AbortError") return;
      setMsg(`无法调起系统分享。文件已保存在应用内:\n${filename}\n可在「文件」App 的本应用目录中找到后发送。`);
    } finally {
      setSharing(false);
    }
  }, [share.path, filename]);

  const doCopyPass = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(share.passphrase);
    } catch {
      // WKWebView 里 clipboard API 偶尔不可用,回退到 execCommand。
      const ta = document.createElement("textarea");
      ta.value = share.passphrase;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      try {
        document.execCommand("copy");
      } catch {
        /* 忽略 */
      }
      document.body.removeChild(ta);
    }
    setCopied(true);
    setTimeout(() => setCopied(false), 1600);
  }, [share.passphrase]);

  return (
    <div className="scrim" onClick={onClose}>
      <div className="dialog share" onClick={(e) => e.stopPropagation()}>
        <div className="share-h">
          <span className="share-badge"><ShieldIcon /></span>
          <div>
            <h3>加密分享已生成</h3>
            <span className="share-sub">端到端加密 · {share.record_count} 份记录</span>
          </div>
        </div>

        <p className="share-tip">
          把<b>文件</b>发给医生,<b>口令另发</b>(用不同渠道,如口令走短信、文件走微信);
          医生打开文件、输入口令即可查看。数据端到端加密,<b>不经服务器</b>。
        </p>

        <div className="share-pass">
          <span className="k">口令</span>
          <span className="v">{share.passphrase}</span>
        </div>

        <div className="share-acts">
          <button className="primary" onClick={doShareFile} disabled={sharing}>
            <ShareIcon />
            {sharing ? "调起分享…" : "分享文件"}
          </button>
          <button className="second" onClick={doCopyPass}>
            <CopyIcon />
            {copied ? "已复制" : "复制口令"}
          </button>
        </div>

        {msg && <div className="share-msg">{msg}</div>}

        <button className="full" onClick={onClose}>
          完成
        </button>
      </div>
    </div>
  );
}

// —— 档案详情:把识别文本渲染得「整洁易读」(轻量启发式,非完整解析器) ——
// 规则:【…】/短标题行 → 小节标题;化验单的「项目 结果 参考范围」行 → 三列对齐
// (值用等宽 + tabular-nums 对齐,异常↑↓染色);处方的编号药品行 → 药品条目;
// 其余 → 正常段落(保留换行、舒适行高)。任何行解析失败都安全回退为段落。
type LabRow = { name: string; value: string; ref: string; flag: "hi" | "lo" | "" };

function parseLabRow(line: string): LabRow | null {
  const cells = line.trim().split(/\s{2,}/).filter(Boolean);
  if (cells.length < 2) return null;
  const idx = cells.findIndex((c) => /^[<>]?\s*-?\d+(\.\d+)?$/.test(c.trim()));
  if (idx <= 0) return null; // 第一格必须是名称,不能是数值
  const name = cells.slice(0, idx).join(" ");
  const value = cells[idx].trim();
  const rest = cells.slice(idx + 1).join(" ");
  const tail = line;
  const flag: LabRow["flag"] = /↑|偏高|升高/.test(tail)
    ? "hi"
    : /↓|偏低|降低/.test(tail)
      ? "lo"
      : "";
  const ref = rest.replace(/[↑↓]/g, "").trim();
  return { name, value, ref, flag };
}

function ClinicalText({ text, docType }: { text: string; docType: string }) {
  const raw = text.replace(/\r\n/g, "\n");
  if (!raw.trim()) {
    return (
      <div className="doc-body">
        <span className="muted">此文件尚未识别出文字。原始文件已完整保存,可点「查看原件」出示给医生。</span>
      </div>
    );
  }
  const lines = raw.split("\n");
  const nodes: ReactNode[] = [];
  let para: string[] = [];
  let seq = 0;
  // 结论/印象类小节(诊断意见、结论、提示等):患者往往只看结论,单独提炼成醒目卡片。
  // 命中该小节标题后,直到下一个小节标题前的内容都归入卡片;其余渲染方式不变。
  let conclusionBuf: ReactNode[] | null = null;
  const active = () => conclusionBuf ?? nodes;
  const isConclusionHeader = (label: string) => /诊断|印象|结论|提示/.test(label) || /意见$/.test(label);

  const flushPara = () => {
    if (para.length) {
      active().push(
        <p className="doc-p" key={`p${seq++}`}>
          {para.join("\n")}
        </p>,
      );
      para = [];
    }
  };
  const flushConclusion = () => {
    if (conclusionBuf) {
      nodes.push(
        <div className="doc-conclusion" key={`c${seq++}`}>
          {conclusionBuf}
        </div>,
      );
      conclusionBuf = null;
    }
  };

  lines.forEach((line) => {
    const t = line.trim();
    if (!t) {
      flushPara();
      return;
    }
    // 小节标题:【…】 或以冒号结尾的短标题
    if (/^【.+】$/.test(t) || (t.length <= 12 && /[::]$/.test(t))) {
      flushPara();
      const label = t.replace(/^【|】$/g, "").replace(/[::]$/, "");
      flushConclusion(); // 上一个结论块(若有)到此结束
      if (isConclusionHeader(label)) {
        conclusionBuf = [];
      }
      active().push(
        <div className="doc-h" key={`h${seq++}`}>
          {label}
        </div>,
      );
      return;
    }
    // 化验单:三列对齐行
    if (docType === "lab_report") {
      const row = parseLabRow(line);
      if (row) {
        flushPara();
        active().push(
          <div className="lab-row" key={`l${seq++}`}>
            <span className="nm">{row.name}</span>
            <span className={`val ${row.flag}`}>
              {row.value}
              {row.flag === "hi" ? " ↑" : row.flag === "lo" ? " ↓" : ""}
            </span>
            <span className="ref">{row.ref}</span>
          </div>,
        );
        return;
      }
    }
    // 处方:编号药品行 → 加粗条目;其后「用法…」缩进行归入正常段落。
    if (docType === "prescription" && /^\d+\.\s/.test(t)) {
      flushPara();
      active().push(
        <div className="doc-med" key={`m${seq++}`}>
          {t}
        </div>,
      );
      return;
    }
    para.push(line);
  });
  flushPara();
  flushConclusion();

  return <div className="doc-body">{nodes}</div>;
}

// 文档详情:类型/日期/来源 + 识别文本;图片文档额外渲染缩略图。
// 无 DICOM 阅片(交给桌面/在线查看器),影像文档只展示文本与元信息。
function DetailScreen({ id, onBack }: { id: number; onBack: () => void }) {
  const [detail, setDetail] = useState<DocumentDetail | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [imgUrl, setImgUrl] = useState<string | null>(null);
  // 「查看原件」全屏预览:图片直接看原图,PDF/其他用 iframe 内联渲染原件。
  const [viewer, setViewer] = useState(false);
  const [origUrl, setOrigUrl] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .getDocument(id)
      .then((d) => {
        if (!cancelled) setDetail(d);
      })
      .catch((e) => {
        if (!cancelled) setErr(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [id]);

  // 图片文档:读取原始字节渲染缩略图。PDF/DICOM 不在手机端内联渲染。
  useEffect(() => {
    if (!detail) return;
    if (!detail.source_file.mime_type.startsWith("image/")) return;
    let cancelled = false;
    let url: string | null = null;
    api
      .readSourceBytes(detail.source_file.id)
      .then((bytes) => {
        if (cancelled) return;
        const blob = new Blob([bytes], { type: detail.source_file.mime_type });
        url = URL.createObjectURL(blob);
        setImgUrl(url);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      if (url) URL.revokeObjectURL(url);
    };
  }, [detail]);

  const doc = detail?.document;
  const sf = detail?.source_file;
  const isImage = sf?.mime_type.startsWith("image/") ?? false;
  const typeLabel = doc ? DOC_LABEL[doc.doc_type] ?? doc.doc_type : "";

  // OCR 置信度:Apple Vision 识别照片时给出;换算成患者能看懂的三档,而非裸百分比。
  const conf = detail?.ocr_confidence ?? null;
  const confTier =
    conf == null ? null : conf >= 0.9 ? "high" : conf >= 0.75 ? "mid" : "low";

  // 查看原件:图片复用已加载的原图 URL;PDF/其他按需读字节生成 blob 供 iframe 预览。
  const openOriginal = useCallback(async () => {
    if (!sf) return;
    if (isImage && imgUrl) {
      setOrigUrl(imgUrl);
      setViewer(true);
      return;
    }
    try {
      const bytes = await api.readSourceBytes(sf.id);
      const blob = new Blob([bytes], { type: sf.mime_type || "application/octet-stream" });
      setOrigUrl(URL.createObjectURL(blob));
      setViewer(true);
    } catch (e) {
      alert(`打开原件失败:${e}`);
    }
  }, [sf, isImage, imgUrl]);

  const closeViewer = useCallback(() => {
    setViewer(false);
    setOrigUrl((u) => {
      if (u && u !== imgUrl) URL.revokeObjectURL(u); // 图片 URL 由缩略图 effect 统一回收
      return null;
    });
  }, [imgUrl]);

  return (
    <div className="app">
      <div className="appbar">
        <button className="backbtn" onClick={onBack}>
          <ArrowLeftIcon />
          返回
        </button>
      </div>
      <div className="body">
        {err ? (
          <div className="empty">打开失败:{err}</div>
        ) : !detail ? (
          <div className="empty">加载中…</div>
        ) : (
          <>
            <div className="dhead">
              <span className={`dic t-${doc!.doc_type}`}>
                <DocTypeIcon type={doc!.doc_type} />
              </span>
              <div className="dmeta">
                <b>{doc!.title ?? typeLabel}</b>
                <span>
                  {typeLabel}
                  {doc!.doc_date ? ` · ${fmtDate(doc!.doc_date)}` : ""}
                </span>
                <span className="src">来源:{sf!.original_name}</span>
              </div>
            </div>

            {/* OCR 置信度徽标:三档(高/中/低)比裸百分比更易懂;悬浮说明识别原理。 */}
            {confTier != null && (
              <div
                className={`conf ${confTier}`}
                title="识别由 AI 自动完成,个别文字可能不准,但大部分是准确的。可点『查看原件』核对。"
              >
                {confTier === "high" ? <CheckCircleIcon /> : <AlertTriangleIcon />}
                <span>
                  {confTier === "high" && "识别质量:高"}
                  {confTier === "mid" && (
                    <>
                      识别质量:中<b> · 个别字可能有误,可核对原件</b>
                    </>
                  )}
                  {confTier === "low" && (
                    <>
                      识别质量:低<b> · 建议重新拍摄</b>
                    </>
                  )}
                </span>
              </div>
            )}

            {/* 查看原件:让用户对照原始照片/文件,核对不确定的识别文字。 */}
            <button className="viewbtn" onClick={openOriginal}>
              <EyeIcon />
              查看原件
            </button>

            {isImage && (
              <div className="dimg">
                {imgUrl ? <img src={imgUrl} alt={sf!.original_name} /> : <div className="empty">加载原图…</div>}
              </div>
            )}

            <div className="sect">
              <FileTextIcon />
              <span style={{ marginLeft: 6 }}>{isImage ? "识别文本" : "文档内容"}</span>
            </div>
            <ClinicalText text={detail.ocr_text} docType={doc!.doc_type} />
            {/* TODO(M2):文本纠错编辑 —— 允许用户就地修正个别识错的字。 */}
          </>
        )}
      </div>

      {/* 全屏原件预览 */}
      {viewer && origUrl && (
        <div className="viewer" onClick={closeViewer}>
          <button className="vclose" onClick={closeViewer}>
            <ArrowLeftIcon />
            关闭
          </button>
          {isImage ? (
            <img src={origUrl} alt="原件" onClick={(e) => e.stopPropagation()} />
          ) : (
            <iframe src={origUrl} title="原件" onClick={(e) => e.stopPropagation()} />
          )}
        </div>
      )}
    </div>
  );
}
