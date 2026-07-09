import { useCallback, useEffect, useState } from "react";
import "./App.css";
import { api } from "./api";
import type {
  TimelineGroup,
  ImportOutcome,
  ShareResult,
  PatientProfile,
  DocumentDetail,
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
  ImageIcon,
  ArrowLeftIcon,
  LinkIcon,
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

export default function App() {
  const [tab, setTab] = useState<Tab>("capture");
  const [groups, setGroups] = useState<TimelineGroup[]>([]);
  const [profile, setProfile] = useState<PatientProfile | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [lastImport, setLastImport] = useState<ImportOutcome | null>(null);
  const [share, setShare] = useState<ShareResult | null>(null);
  const [confirmReset, setConfirmReset] = useState(false);
  const [version, setVersion] = useState("");
  // 点开的文档 id —— 非空时全屏展示文档详情(见 DetailScreen)。
  const [detailId, setDetailId] = useState<number | null>(null);
  // 就诊组在档案里点开时展开其子文档(每份可再点开详情)。
  const [expanded, setExpanded] = useState<Set<number>>(new Set());

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

  // 采集:通过系统相册/文件选择器拿到沙盒可读路径,交给 pipeline ingest。
  // 说明:iOS 上用 tauri-plugin-dialog 的 open() 打开原生选择器;插件会把选中的
  // 文件拷进 App 缓存目录并返回沙盒内可读路径,ingest 直接读取。
  // filters 只保留有稳定 UTType 映射的图片/PDF —— 之前带 "dcm" 会让 iOS 的
  // UIDocumentPicker 因无法解析 UTType 而整体失败(采集点了没反应的根因);
  // DICOM 阅片本就交给桌面/在线查看器,手机端不需要。
  // 真正的「相机内实时拍摄」需相机插件(原生),列为后续跟进。
  const capture = useCallback(async () => {
    setShare(null);
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const picked = await open({
        multiple: false,
        title: "拍照 / 从相册或文件选择",
        filters: [
          { name: "照片 / 报告", extensions: ["jpg", "jpeg", "png", "heic", "heif", "pdf"] },
        ],
      });
      if (!picked || Array.isArray(picked)) return;
      const path = typeof picked === "string" ? picked : (picked as { path: string }).path;
      setBusy("正在识别并入库…");
      const outcome = await api.ingestFile(path);
      setLastImport(outcome);
      await refresh();
    } catch (e) {
      console.error("capture failed", e);
      alert(`采集失败:${e}`);
    } finally {
      setBusy(null);
    }
  }, [refresh]);

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
          <span className="logo">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" />
              <path d="M9 12l2 2 4-4" />
            </svg>
          </span>
          医我
        </div>
        <div className="who">{initial}</div>
      </div>

      {tab === "capture" ? (
        <div className="body">
          <button className="shoot" onClick={capture} disabled={!!busy}>
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

          {/* 同步:v1.1 iCloud container 之前的手动方案指引 */}
          <div className="sect">同步</div>
          <div className="group">
            <div className="info">
              把「保险箱」放进你自己的云盘即可与桌面自动同步:
              <b>苹果用户用 iCloud 云盘,安卓/其他用坚果云</b>。
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

      {/* 加密分享结果(M1 简版):展示口令 + 落盘路径。系统「分享」sheet 导出 = M2。 */}
      {share && (
        <div className="toast" onClick={() => setShare(null)}>
          <div className="h ok">
            <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round">
              <path d="M20 6L9 17l-5-5" />
            </svg>
            已生成 · 端到端加密 · {share.record_count} 份
          </div>
          <div className="copyline">
            <span className="k">口令</span>
            <span className="v">{share.passphrase}</span>
          </div>
          <div className="note">
            数据在对方浏览器本地解密,不经服务器。文件已存到:
            <br />
            <small>{share.path}</small>
            <br />
            <small>系统「分享」导出为 M2</small>
          </div>
        </div>
      )}

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

// 文档详情:类型/日期/来源 + 识别文本;图片文档额外渲染缩略图。
// 无 DICOM 阅片(交给桌面/在线查看器),影像文档只展示文本与元信息。
function DetailScreen({ id, onBack }: { id: number; onBack: () => void }) {
  const [detail, setDetail] = useState<DocumentDetail | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [imgUrl, setImgUrl] = useState<string | null>(null);

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

            {isImage && (
              <div className="dimg">
                {imgUrl ? <img src={imgUrl} alt={sf!.original_name} /> : <div className="empty">加载原图…</div>}
              </div>
            )}

            <div className="sect">
              <ImageIcon />
              <span style={{ marginLeft: 6 }}>{isImage ? "识别文本" : "文档内容"}</span>
            </div>
            <div className="dtext">
              {detail.ocr_text.trim() ? (
                detail.ocr_text
              ) : (
                <span className="muted">此文件尚未识别出文字。原始文件已完整保存,可直接出示给医生。</span>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
