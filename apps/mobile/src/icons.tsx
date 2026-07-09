// 内联 SVG 图标(lucide 风格),替代 emoji —— iOS WKWebView 里 emoji 会渲染成
// “?” 缺字框且对不齐;内联 SVG 用 currentColor 描边,清晰且随容器居中对齐。
// 每个文档类型一枚一眼可辨的图标,与桌面端 docmeta.ts 的 lucide 映射保持一致。
import type { ReactNode } from "react";

function Svg({ children }: { children: ReactNode }) {
  return (
    <svg
      viewBox="0 0 24 24"
      width="1em"
      height="1em"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {children}
    </svg>
  );
}

// —— 文档类型图标 ——
export const FlaskIcon = () => (
  <Svg>
    <path d="M14 2v6a2 2 0 0 0 .245.96l5.51 10.08A2 2 0 0 1 18 22H6a2 2 0 0 1-1.755-2.96l5.51-10.08A2 2 0 0 0 10 8V2" />
    <path d="M6.453 15h11.094" />
    <path d="M8.5 2h7" />
  </Svg>
);
export const ScanIcon = () => (
  <Svg>
    <path d="M3 7V5a2 2 0 0 1 2-2h2" />
    <path d="M17 3h2a2 2 0 0 1 2 2v2" />
    <path d="M21 17v2a2 2 0 0 1-2 2h-2" />
    <path d="M7 21H5a2 2 0 0 1-2-2v-2" />
    <path d="M7 12h10" />
  </Svg>
);
export const StethoscopeIcon = () => (
  <Svg>
    <path d="M11 2v2" />
    <path d="M5 2v2" />
    <path d="M5 3H4a2 2 0 0 0-2 2v4a6 6 0 0 0 12 0V5a2 2 0 0 0-2-2h-1" />
    <path d="M8 15a6 6 0 0 0 12 0v-3" />
    <circle cx="20" cy="10" r="2" />
  </Svg>
);
export const PillIcon = () => (
  <Svg>
    <path d="m10.5 20.5 10-10a4.95 4.95 0 1 0-7-7l-10 10a4.95 4.95 0 1 0 7 7Z" />
    <path d="m8.5 8.5 7 7" />
  </Svg>
);
export const MicroscopeIcon = () => (
  <Svg>
    <path d="M6 18h8" />
    <path d="M3 22h18" />
    <path d="M14 22a7 7 0 1 0 0-14h-1" />
    <path d="M9 14h2" />
    <path d="M9 12a2 2 0 0 1-2-2V6h6v4a2 2 0 0 1-2 2Z" />
    <path d="M12 6V3a1 1 0 0 0-1-1H9a1 1 0 0 0-1 1v3" />
  </Svg>
);
export const ScissorsIcon = () => (
  <Svg>
    <circle cx="6" cy="6" r="3" />
    <path d="M8.12 8.12 12 12" />
    <path d="M20 4 8.12 15.88" />
    <circle cx="6" cy="18" r="3" />
    <path d="M14.8 14.8 20 20" />
  </Svg>
);
export const BedIcon = () => (
  <Svg>
    <path d="M2 20v-8a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2v8" />
    <path d="M4 10V6a2 2 0 0 1 2-2h12a2 2 0 0 1 2 2v4" />
    <path d="M12 4v6" />
    <path d="M2 18h20" />
  </Svg>
);
export const FileTextIcon = () => (
  <Svg>
    <path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7z" />
    <path d="M14 2v5h5" />
    <path d="M16 13H8" />
    <path d="M16 17H8" />
    <path d="M10 9H8" />
  </Svg>
);
export const FileQuestionIcon = () => (
  <Svg>
    <path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7z" />
    <path d="M14 2v5h5" />
    <path d="M9.5 11a2.5 2.5 0 0 1 4.6 1.2c0 1.5-2 2-2 2.8" />
    <path d="M12 18h.01" />
  </Svg>
);
export const HospitalIcon = () => (
  <Svg>
    <path d="M12 6v4" />
    <path d="M14 8h-4" />
    <path d="M18 12h2a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2v-6a2 2 0 0 1 2-2h2" />
    <path d="M18 22V4a2 2 0 0 0-2-2H8a2 2 0 0 0-2 2v18" />
  </Svg>
);

// —— 功能/状态图标 ——
export const FolderIcon = () => (
  <Svg>
    <path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" />
  </Svg>
);
export const DownloadIcon = () => (
  <Svg>
    <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
    <path d="m7 10 5 5 5-5" />
    <path d="M12 15V3" />
  </Svg>
);
export const TrashIcon = () => (
  <Svg>
    <path d="M3 6h18" />
    <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
    <path d="M10 11v6" />
    <path d="M14 11v6" />
  </Svg>
);
export const AlertTriangleIcon = () => (
  <Svg>
    <path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3Z" />
    <path d="M12 9v4" />
    <path d="M12 17h.01" />
  </Svg>
);
export const CheckCircleIcon = () => (
  <Svg>
    <path d="M22 11.08V12a10 10 0 1 1-5.93-9.14" />
    <path d="m9 11 3 3L22 4" />
  </Svg>
);
export const ImageIcon = () => (
  <Svg>
    <rect width="18" height="18" x="3" y="3" rx="2" ry="2" />
    <circle cx="9" cy="9" r="2" />
    <path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21" />
  </Svg>
);
export const ArrowLeftIcon = () => (
  <Svg>
    <path d="m12 19-7-7 7-7" />
    <path d="M19 12H5" />
  </Svg>
);
export const LinkIcon = () => (
  <Svg>
    <path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" />
    <path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" />
  </Svg>
);

// 文档类型 → 图标组件(与桌面 docmeta.ts TYPE_ICON 对应)。
const DOC_ICON: Record<string, () => ReactNode> = {
  lab_report: FlaskIcon,
  imaging_report: ScanIcon,
  prescription: PillIcon,
  discharge_summary: BedIcon,
  clinical_note: StethoscopeIcon,
  pathology: MicroscopeIcon,
  surgery: ScissorsIcon,
  other: FileTextIcon,
  unknown: FileQuestionIcon,
};

// 就诊类型 → 图标(门诊=听诊器、急诊/住院/体检等统一用医院图标兜底)。
const KIND_ICON: Record<string, () => ReactNode> = {
  outpatient: StethoscopeIcon,
  inpatient: BedIcon,
};

export function DocTypeIcon({ type }: { type: string }) {
  const C = DOC_ICON[type] ?? FileTextIcon;
  return <C />;
}

export function EncounterIcon({ kind }: { kind: string }) {
  const C = KIND_ICON[kind] ?? HospitalIcon;
  return <C />;
}
