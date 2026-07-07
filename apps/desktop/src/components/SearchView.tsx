import { useState } from "react";
import { Search as SearchIcon, FileQuestion } from "lucide-react";
import { api } from "../api";
import type { SearchResult } from "../types";
import { TYPE_ICON, TYPE_BADGE, fmtDate } from "../docmeta";

// FTS body 是 jieba 分词后文本 → 片段里中文之间有空格;渲染时去掉中文间空格。
function deCjkSpace(s: string): string {
  return s.replace(/([一-鿿])\s+(?=[一-鿿])/g, "$1");
}

// 片段中的 [..] 是命中高亮
function renderSnippet(snippet: string) {
  return snippet.split(/(\[[^\]]*\])/g).map((p, i) => {
    if (p.startsWith("[") && p.endsWith("]")) {
      return (
        <mark key={i} className="bg-amber-100 text-amber-900 rounded px-0.5">
          {deCjkSpace(p.slice(1, -1))}
        </mark>
      );
    }
    return <span key={i}>{deCjkSpace(p)}</span>;
  });
}

export default function SearchView({ onSelect }: { onSelect: (id: number) => void }) {
  const [q, setQ] = useState("");
  const [results, setResults] = useState<SearchResult[] | null>(null);
  const [busy, setBusy] = useState(false);

  const run = (query: string) => {
    setQ(query);
    if (!query.trim()) {
      setResults(null);
      return;
    }
    setBusy(true);
    api
      .search(query, 50)
      .then(setResults)
      .catch(() => setResults([]))
      .finally(() => setBusy(false));
  };

  return (
    <div className="flex-1 overflow-y-auto bg-slate-50 p-6 md:p-10">
      <div className="max-w-3xl mx-auto">
        <h1 className="text-2xl font-bold text-slate-900 mb-6">搜索</h1>
        <div className="relative">
          <SearchIcon className="w-5 h-5 text-slate-400 absolute left-4 top-1/2 -translate-y-1/2" />
          <input
            autoFocus
            value={q}
            onChange={(e) => run(e.target.value)}
            placeholder="搜索肌酐、Metoprolol、脂肪肝、CT、胆囊…"
            className="w-full pl-12 pr-4 py-3 rounded-2xl border border-slate-200 bg-white text-base focus:outline-none focus:border-blue-400 focus:ring-2 focus:ring-blue-100"
          />
        </div>

        {results !== null && (
          <div className="mt-6 space-y-2">
            <div className="text-[11px] font-mono text-slate-400 uppercase tracking-widest">
              {busy ? "搜索中…" : `${results.length} 条结果`}
            </div>
            {results.map((r) => {
              const d = r.document;
              const Icon = TYPE_ICON[d.doc_type] ?? FileQuestion;
              return (
                <button
                  key={d.id}
                  onClick={() => onSelect(d.id)}
                  className="w-full text-left bg-white border border-slate-200 rounded-2xl p-4 shadow-sm hover:shadow-md transition-all cursor-pointer group"
                >
                  <div className="flex items-center gap-3">
                    <div
                      className={`w-9 h-9 rounded-lg flex items-center justify-center shrink-0 ${
                        TYPE_BADGE[d.doc_type] ?? "bg-slate-100 text-slate-600"
                      }`}
                    >
                      <Icon className="w-4 h-4" />
                    </div>
                    <span className="font-medium text-slate-800 group-hover:text-blue-700 truncate flex-1">
                      {d.title ?? "(无标题)"}
                    </span>
                    <span className="text-xs font-mono text-slate-500 shrink-0">
                      {fmtDate(d.doc_date)}
                    </span>
                  </div>
                  <div className="text-sm text-slate-600 mt-2 leading-relaxed line-clamp-2">
                    {renderSnippet(r.snippet)}
                  </div>
                </button>
              );
            })}
            {!busy && results.length === 0 && (
              <div className="text-slate-400 text-sm">没有匹配的记录。</div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
