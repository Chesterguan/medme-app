import { useState } from "react";
import {
  ChevronDown,
  ChevronRight,
  FileQuestion,
  Stethoscope,
  ArrowLeftRight,
} from "lucide-react";
import type { TimelineGroup, DocumentSummary, EncounterSummary } from "../types";
import {
  TYPE_LABEL,
  TYPE_ACCENT,
  TYPE_BADGE,
  TYPE_ICON,
  KIND_LABEL,
  KIND_ICON,
  KIND_TINT,
  fmtDate,
} from "../docmeta";

function docDateStr(d: DocumentSummary): string {
  return d.doc_date_end
    ? `${fmtDate(d.doc_date)} → ${fmtDate(d.doc_date_end)}`
    : fmtDate(d.doc_date);
}

// 独立文档:大卡片(与就诊组同层级)
function DocCard({ d, onSelect }: { d: DocumentSummary; onSelect: (id: number) => void }) {
  const Icon = TYPE_ICON[d.doc_type] ?? FileQuestion;
  return (
    <button
      onClick={() => onSelect(d.id)}
      className={`w-full text-left bg-white border border-slate-200 border-l-4 ${
        TYPE_ACCENT[d.doc_type] ?? "border-slate-300"
      } rounded-2xl p-4 shadow-sm hover:shadow-md hover:-translate-y-0.5 transition-all cursor-pointer group`}
    >
      <div className="flex items-center gap-4">
        <div
          className={`w-11 h-11 rounded-xl flex items-center justify-center shrink-0 ${
            TYPE_BADGE[d.doc_type] ?? "bg-slate-100 text-slate-600"
          }`}
        >
          <Icon className="w-5 h-5" />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-start justify-between gap-4">
            <span className="font-medium text-base text-slate-800 group-hover:text-blue-700 transition-colors truncate">
              {d.title ?? "(无标题)"}
            </span>
            <span className="text-sm font-mono text-slate-500 shrink-0 pt-0.5">{docDateStr(d)}</span>
          </div>
          <span
            className={`text-xs font-mono px-2 py-0.5 rounded-full mt-1.5 inline-block ${
              TYPE_BADGE[d.doc_type] ?? "bg-slate-100 text-slate-600"
            }`}
          >
            {TYPE_LABEL[d.doc_type] ?? d.doc_type}
          </span>
        </div>
      </div>
    </button>
  );
}

// 就诊组内的文档行:紧凑
function DocRow({ d, onSelect }: { d: DocumentSummary; onSelect: (id: number) => void }) {
  const Icon = TYPE_ICON[d.doc_type] ?? FileQuestion;
  return (
    <button
      onClick={() => onSelect(d.id)}
      className="w-full text-left flex items-center gap-3 px-3 py-2 rounded-xl hover:bg-white transition-colors cursor-pointer group"
    >
      <div
        className={`w-8 h-8 rounded-lg flex items-center justify-center shrink-0 ${
          TYPE_BADGE[d.doc_type] ?? "bg-slate-100 text-slate-600"
        }`}
      >
        <Icon className="w-4 h-4" />
      </div>
      <span className="text-sm text-slate-700 group-hover:text-blue-700 truncate flex-1">
        {d.title ?? "(无标题)"}
      </span>
      <span className="text-[11px] font-mono text-slate-400 shrink-0">
        {TYPE_LABEL[d.doc_type] ?? d.doc_type}
      </span>
      <span className="text-xs font-mono text-slate-500 shrink-0 w-24 text-right">
        {fmtDate(d.doc_date)}
      </span>
    </button>
  );
}

// 就诊组:可展开
function EncounterCard({
  enc,
  docs,
  onSelect,
}: {
  enc: EncounterSummary;
  docs: DocumentSummary[];
  onSelect: (id: number) => void;
}) {
  const [open, setOpen] = useState(false);
  const KindIcon = KIND_ICON[enc.kind] ?? Stethoscope;
  const dateStr = enc.end_date
    ? `${fmtDate(enc.start_date)} → ${fmtDate(enc.end_date)}`
    : fmtDate(enc.start_date);
  return (
    <div className="bg-white border border-slate-200 rounded-2xl shadow-sm overflow-hidden">
      <button
        onClick={() => setOpen((o) => !o)}
        className="w-full text-left flex items-center gap-4 p-4 hover:bg-slate-50/60 transition-colors cursor-pointer"
      >
        {open ? (
          <ChevronDown className="w-5 h-5 text-slate-400 shrink-0" />
        ) : (
          <ChevronRight className="w-5 h-5 text-slate-400 shrink-0" />
        )}
        <div
          className={`w-11 h-11 rounded-xl flex items-center justify-center shrink-0 ${
            KIND_TINT[enc.kind] ?? "bg-slate-100 text-slate-600"
          }`}
        >
          <KindIcon className="w-5 h-5" />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2 flex-wrap">
            <span className="font-semibold text-base text-slate-900">
              {KIND_LABEL[enc.kind] ?? enc.kind}
            </span>
            {enc.provider && <span className="text-sm text-slate-600">· {enc.provider}</span>}
            {enc.transferred && (
              <span className="text-[11px] font-mono px-1.5 py-0.5 rounded-full bg-amber-50 text-amber-700 flex items-center gap-1">
                <ArrowLeftRight className="w-3 h-3" />
                转院
              </span>
            )}
          </div>
          <span className="text-xs font-mono text-slate-400">{enc.doc_count} 份记录</span>
        </div>
        <span className="text-sm font-mono text-slate-500 shrink-0">{dateStr}</span>
      </button>
      {open && (
        <div className="border-t border-slate-100 p-2 space-y-0.5 bg-slate-50/40">
          {docs.map((d) => (
            <DocRow key={d.id} d={d} onSelect={onSelect} />
          ))}
        </div>
      )}
    </div>
  );
}

export default function Timeline({
  groups,
  onSelect,
}: {
  groups: TimelineGroup[];
  onSelect: (id: number) => void;
}) {
  if (groups.length === 0) {
    return (
      <div className="flex-1 flex items-center justify-center text-slate-400 text-base px-6 text-center">
        还没有记录。导入病历后,这里会按时间显示你的生命时间线。
      </div>
    );
  }
  const total = groups.reduce(
    (n, g) => n + (g.group_type === "encounter" ? g.encounter.doc_count : 1),
    0,
  );
  const visits = groups.filter((g) => g.group_type === "encounter").length;
  return (
    <div className="flex-1 overflow-y-auto bg-slate-50 p-6 md:p-10">
      <div className="max-w-4xl mx-auto space-y-4">
        <h1 className="text-2xl font-bold text-slate-900 mb-6">
          生命时间线
          <span className="ml-2 text-sm font-mono text-slate-500">
            {total} 份 · {visits} 次就诊
          </span>
        </h1>
        {groups.map((g) =>
          g.group_type === "encounter" ? (
            <EncounterCard
              key={`e${g.encounter.id}`}
              enc={g.encounter}
              docs={g.docs}
              onSelect={onSelect}
            />
          ) : (
            <DocCard key={`d${g.doc.id}`} d={g.doc} onSelect={onSelect} />
          ),
        )}
      </div>
    </div>
  );
}
