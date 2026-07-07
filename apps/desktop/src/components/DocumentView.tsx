import { useEffect, useState } from "react";
import { ArrowLeft, FileType2, ImageIcon, X, Maximize2, FileQuestion } from "lucide-react";
import { api } from "../api";
import type { DocumentDetail } from "../types";
import { TYPE_LABEL, TYPE_BADGE, TYPE_ICON, fmtDate, fmtBytes } from "../docmeta";
import ReportContent from "./ReportContent";

// 内容(识别文本)为主,原件作为附件:缩略图/文件条,点击全屏查看。
// OCR 已把内容读出来 → 阅读用文本,原图只在需要出示时全屏打开。
export default function DocumentView({
  detail,
  onBack,
}: {
  detail: DocumentDetail;
  onBack: () => void;
}) {
  const { document: doc, source_file: sf, ocr_text } = detail;
  const [origUrl, setOrigUrl] = useState<string | null>(null);
  const [lightbox, setLightbox] = useState(false);
  const isImage = sf.mime_type.startsWith("image/");
  const isPdf = sf.mime_type === "application/pdf";
  const hasOriginal = isImage || isPdf;

  useEffect(() => {
    if (!hasOriginal) return;
    let url: string | null = null;
    api
      .readSourceBytes(doc.id)
      .then((bytes) => {
        const blob = new Blob([new Uint8Array(bytes)], { type: sf.mime_type });
        url = URL.createObjectURL(blob);
        setOrigUrl(url);
      })
      .catch(() => {});
    return () => {
      if (url) URL.revokeObjectURL(url);
    };
  }, [doc.id, hasOriginal, sf.mime_type]);

  const dateStr = doc.doc_date_end
    ? `${fmtDate(doc.doc_date)} → ${fmtDate(doc.doc_date_end)}`
    : fmtDate(doc.doc_date);
  const TypeIcon = TYPE_ICON[doc.doc_type] ?? FileQuestion;

  return (
    <div className="flex-1 flex flex-col h-full overflow-hidden bg-slate-50">
      {/* header */}
      <div className="px-6 md:px-10 py-5 border-b border-slate-200 bg-white/80 backdrop-blur shrink-0">
        <button
          onClick={onBack}
          className="flex items-center gap-1.5 text-sm text-slate-500 hover:text-slate-900 mb-3 cursor-pointer"
        >
          <ArrowLeft className="w-4 h-4" /> 返回
        </button>
        <div className="flex items-center gap-3 flex-wrap">
          <div
            className={`w-9 h-9 rounded-lg flex items-center justify-center shrink-0 ${
              TYPE_BADGE[doc.doc_type] ?? "bg-slate-100 text-slate-600"
            }`}
          >
            <TypeIcon className="w-5 h-5" />
          </div>
          <h1 className="text-2xl font-bold text-slate-900">{doc.title ?? "(无标题)"}</h1>
          <span
            className={`text-xs font-mono px-2.5 py-1 rounded-full ${
              TYPE_BADGE[doc.doc_type] ?? "bg-slate-100 text-slate-600"
            }`}
          >
            {TYPE_LABEL[doc.doc_type] ?? doc.doc_type}
          </span>
          <span className="text-sm font-mono text-slate-500">{dateStr}</span>
        </div>
        <div className="mt-2 text-xs font-mono text-slate-400 flex flex-wrap gap-x-4 gap-y-1">
          <span>原始文件:{sf.original_name}</span>
          <span>{sf.mime_type}</span>
          <span>{fmtBytes(sf.byte_size)}</span>
          <span>导入 {fmtDate(sf.imported_at)}</span>
        </div>
      </div>

      {/* 主滚动区:原件附件 + 识别文本 */}
      <div className="flex-1 overflow-y-auto p-6 md:p-10">
        <div className="max-w-3xl mx-auto space-y-6">
          {/* 原件 · 附件 */}
          {hasOriginal && (
            <div>
              <div className="text-[11px] font-mono text-slate-400 uppercase tracking-widest mb-2">
                原件 · 附件
              </div>
              {isImage ? (
                origUrl ? (
                  <button
                    onClick={() => setLightbox(true)}
                    className="group relative block rounded-xl overflow-hidden border border-slate-200 shadow-sm hover:shadow-md transition-all cursor-zoom-in bg-white"
                  >
                    <img
                      src={origUrl}
                      alt={sf.original_name}
                      className="max-h-80 w-auto mx-auto"
                    />
                    <div className="absolute top-2 right-2 bg-black/50 text-white rounded-lg px-2 py-1 text-xs flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
                      <Maximize2 className="w-3.5 h-3.5" /> 查看大图
                    </div>
                  </button>
                ) : (
                  <div className="text-slate-400 text-sm">加载原件…</div>
                )
              ) : (
                <button
                  onClick={() => setLightbox(true)}
                  className="flex items-center gap-3 bg-white border border-slate-200 rounded-xl px-4 py-3 shadow-sm hover:shadow-md transition-all cursor-pointer w-full text-left"
                >
                  <div className="w-10 h-10 rounded-lg bg-rose-50 text-rose-600 flex items-center justify-center shrink-0">
                    <FileType2 className="w-5 h-5" />
                  </div>
                  <div className="min-w-0 flex-1">
                    <div className="text-sm font-medium text-slate-800 truncate">
                      {sf.original_name}
                    </div>
                    <div className="text-xs font-mono text-slate-400">
                      PDF · {fmtBytes(sf.byte_size)} · 点击全屏查看
                    </div>
                  </div>
                  <Maximize2 className="w-4 h-4 text-slate-400 shrink-0" />
                </button>
              )}
            </div>
          )}

          {/* 识别文本 / 文档内容(主) */}
          <div>
            <div className="text-[11px] font-mono text-slate-400 uppercase tracking-widest mb-2 flex items-center gap-1.5">
              {hasOriginal ? (
                <>
                  <ImageIcon className="w-3.5 h-3.5" /> 识别文本 · 可溯源
                </>
              ) : (
                "文档内容 · 原文"
              )}
            </div>
            <div className="bg-white rounded-2xl border border-slate-200 p-6 shadow-sm">
              {ocr_text.trim() ? (
                <ReportContent text={ocr_text} />
              ) : (
                <div className="text-slate-400 text-sm leading-relaxed">
                  此文件尚未识别出文字。原始文件已完整保存(见上方附件),可直接出示给医生。
                </div>
              )}
            </div>
          </div>
        </div>
      </div>

      {/* 全屏查看 lightbox */}
      {lightbox && origUrl && (
        <div className="fixed inset-0 z-50 bg-black/85 flex flex-col" onClick={() => setLightbox(false)}>
          <div className="flex justify-between items-center px-5 py-3 text-white/90 shrink-0">
            <span className="text-sm font-mono truncate">{sf.original_name}</span>
            <button
              onClick={() => setLightbox(false)}
              className="flex items-center gap-1.5 text-sm hover:text-white cursor-pointer"
            >
              关闭 <X className="w-4 h-4" />
            </button>
          </div>
          <div className="flex-1 overflow-auto flex items-center justify-center p-4">
            {isImage ? (
              <img
                src={origUrl}
                alt={sf.original_name}
                className="max-w-full max-h-full object-contain"
                onClick={(e) => e.stopPropagation()}
              />
            ) : (
              <iframe
                src={origUrl}
                title={sf.original_name}
                className="w-full h-full max-w-5xl bg-white rounded-lg"
                onClick={(e) => e.stopPropagation()}
              />
            )}
          </div>
        </div>
      )}
    </div>
  );
}
