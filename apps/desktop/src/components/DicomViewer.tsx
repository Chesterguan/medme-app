import { useEffect, useRef, useState } from "react";
import {
  RenderingEngine,
  Enums as csEnums,
  cache,
  init as coreInit,
  type Types as csTypes,
} from "@cornerstonejs/core";
import {
  init as toolsInit,
  addTool,
  ToolGroupManager,
  StackScrollTool,
  WindowLevelTool,
  ZoomTool,
  PanTool,
  LengthTool,
  Enums as csToolsEnums,
} from "@cornerstonejs/tools";
import * as dicomImageLoader from "@cornerstonejs/dicom-image-loader";
import { SlidersHorizontal, Ruler, RotateCcw } from "lucide-react";

// 交互式 DICOM 查看器(基于 Cornerstone3D / OHIF 引擎,imaging overhaul P2):
// 鼠标滚轮天然逐张滚动序列(不必先选工具)、窗宽窗位预设、缩放平移、长度测量。
// 仅在全屏 lightbox 中挂载;卸载时销毁渲染引擎、工具组并清空图像缓存,避免泄漏。
// 本地字节(无服务器):每张切片的字节 → Blob → wadouri fileManager → `dicomfile:N`
// imageId → 一个 STACK 视口 setStack。压缩(JPEG2000/JPEG-LS 等)在 web worker +
// WASM codec 里解码,由 Cornerstone3D 的图像缓存按需管理。

const { ViewportType, Events: csEvents } = csEnums;
const { MouseBindings } = csToolsEnums;

// 图像缓存上限(014 大数据顾虑):几百切片也只常驻几十 MB,滚动时解新淘旧。
const CACHE_MAX_BYTES = 300 * 1024 * 1024;

// 窗宽窗位预设:center(C)/width(W) → voiRange。默认 = 用 DICOM 自带窗位(resetProperties)。
const PRESETS: { label: string; center: number | null; width: number | null }[] = [
  { label: "默认", center: null, width: null },
  { label: "脑窗", center: 40, width: 80 },
  { label: "骨窗", center: 500, width: 2000 },
  { label: "肺窗", center: -600, width: 1500 },
  { label: "软组织", center: 40, width: 400 },
];

type LeftTool = "WindowLevel" | "Length";

// Cornerstone3D 全局初始化只做一次(core / tools / dicomImageLoader + 工具注册 +
// 缓存上限)。多次挂载共享同一个 promise,避免重复注册 worker / 工具。
let initPromise: Promise<void> | null = null;
function ensureCornerstoneInit(): Promise<void> {
  if (!initPromise) {
    initPromise = (async () => {
      coreInit();
      toolsInit();
      // web worker(Vite `new URL(..., import.meta.url)`)+ WASM codec 解码压缩帧。
      dicomImageLoader.init();
      cache.setMaxCacheSize(CACHE_MAX_BYTES);
      // 工具类全局注册一次;每个查看器实例再各自建 ToolGroup 绑定按键。
      addTool(StackScrollTool);
      addTool(WindowLevelTool);
      addTool(ZoomTool);
      addTool(PanTool);
      addTool(LengthTool);
    })();
  }
  return initPromise;
}

let seq = 0;

export default function DicomViewer({
  slices,
  fileName,
}: {
  slices: Uint8Array[];
  fileName: string;
}) {
  const elementRef = useRef<HTMLDivElement | null>(null);
  const engineRef = useRef<RenderingEngine | null>(null);
  // 每个实例唯一的 id,避免多次挂载/HMR 时渲染引擎与工具组冲突。
  const ids = useRef({
    engine: `dicom-engine-${++seq}`,
    viewport: `dicom-viewport-${seq}`,
    toolGroup: `dicom-toolgroup-${seq}`,
  }).current;

  const [ready, setReady] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [leftTool, setLeftTool] = useState<LeftTool>("WindowLevel");
  const [sliceIndex, setSliceIndex] = useState(0);
  const [sliceTotal, setSliceTotal] = useState(slices.length);

  const getViewport = () =>
    engineRef.current?.getViewport(ids.viewport) as
      | csTypes.IStackViewport
      | undefined;

  // 初始化 + 加载:字节 → imageIds → STACK 视口 → 工具组(滚轮滚动切片)。
  // 畸形 DICOM 可能抛错 —— try/catch 转成内联错误提示,避免冒泡把整个应用白屏
  // (外层还有 ErrorBoundary 兜底)。
  useEffect(() => {
    let disposed = false;
    const element = elementRef.current;
    if (!element) return;
    setReady(false);
    setError(null);
    setSliceTotal(slices.length);
    setSliceIndex(0);

    // fileManager 会一直持有 Blob;记录本次加入的下标,卸载时移除。
    const fileIndices: number[] = [];

    const onStackNewImage = () => {
      const vp = getViewport();
      if (vp) setSliceIndex(vp.getCurrentImageIdIndex());
    };

    (async () => {
      try {
        await ensureCornerstoneInit();
        if (disposed) return;

        // 每张切片:拷贝出独立 ArrayBuffer(Uint8Array 可能是大 buffer 的视图)→
        // Blob → fileManager.add → `dicomfile:N` imageId。切片已在后端按堆栈顺序排好。
        const imageIds = slices.map((bytes) => {
          const blob = new Blob([bytes.slice().buffer], {
            type: "application/dicom",
          });
          const imageId = dicomImageLoader.wadouri.fileManager.add(blob);
          const idx = Number(imageId.split(":")[1]);
          if (!Number.isNaN(idx)) fileIndices.push(idx);
          return imageId;
        });

        const engine = new RenderingEngine(ids.engine);
        engineRef.current = engine;
        engine.enableElement({
          viewportId: ids.viewport,
          type: ViewportType.STACK,
          element,
        });

        const viewport = engine.getViewport(
          ids.viewport
        ) as csTypes.IStackViewport;
        await viewport.setStack(imageIds);
        if (disposed) return;
        viewport.render();

        setSliceTotal(imageIds.length);
        setSliceIndex(viewport.getCurrentImageIdIndex());
        element.addEventListener(csEvents.STACK_NEW_IMAGE, onStackNewImage);

        // 工具组:滚轮 → 序列滚动(主要诉求);左键窗位、右键缩放、中键平移;
        // 长度测量 passive(选中"长度测量"时切到左键)。
        const toolGroup = ToolGroupManager.createToolGroup(ids.toolGroup);
        if (toolGroup) {
          toolGroup.addTool(StackScrollTool.toolName);
          toolGroup.addTool(WindowLevelTool.toolName);
          toolGroup.addTool(ZoomTool.toolName);
          toolGroup.addTool(PanTool.toolName);
          toolGroup.addTool(LengthTool.toolName);
          toolGroup.addViewport(ids.viewport, ids.engine);
          toolGroup.setToolActive(StackScrollTool.toolName, {
            bindings: [{ mouseButton: MouseBindings.Wheel }],
          });
          toolGroup.setToolActive(WindowLevelTool.toolName, {
            bindings: [{ mouseButton: MouseBindings.Primary }],
          });
          toolGroup.setToolActive(ZoomTool.toolName, {
            bindings: [{ mouseButton: MouseBindings.Secondary }],
          });
          toolGroup.setToolActive(PanTool.toolName, {
            bindings: [{ mouseButton: MouseBindings.Auxiliary }],
          });
          toolGroup.setToolPassive(LengthTool.toolName);
        }

        if (!disposed) setReady(true);
      } catch (e) {
        console.error("DICOM 加载失败", e);
        if (!disposed) setError("DICOM 加载失败");
      }
    })();

    return () => {
      disposed = true;
      element.removeEventListener(csEvents.STACK_NEW_IMAGE, onStackNewImage);
      try {
        ToolGroupManager.destroyToolGroup(ids.toolGroup);
      } catch {
        /* 未成功创建时忽略 */
      }
      try {
        engineRef.current?.destroy();
      } catch {
        /* 已销毁时忽略 */
      }
      engineRef.current = null;
      // 释放 fileManager 里本次的 Blob,并清空图像缓存(同一时刻只有一个查看器)。
      fileIndices.forEach((i) => dicomImageLoader.wadouri.fileManager.remove(i));
      try {
        cache.purgeCache();
      } catch {
        /* 忽略 */
      }
    };
  }, [slices, fileName, ids]);

  // 左键工具切换:窗宽窗位 ⇄ 长度测量。两者不能共享主键,故把另一个设为 passive。
  useEffect(() => {
    if (!ready) return;
    const tg = ToolGroupManager.getToolGroup(ids.toolGroup);
    if (!tg) return;
    if (leftTool === "WindowLevel") {
      tg.setToolPassive(LengthTool.toolName);
      tg.setToolActive(WindowLevelTool.toolName, {
        bindings: [{ mouseButton: MouseBindings.Primary }],
      });
    } else {
      tg.setToolPassive(WindowLevelTool.toolName);
      tg.setToolActive(LengthTool.toolName, {
        bindings: [{ mouseButton: MouseBindings.Primary }],
      });
    }
  }, [leftTool, ready, ids.toolGroup]);

  const applyPreset = (center: number | null, width: number | null) => {
    const vp = getViewport();
    if (!vp) return;
    try {
      if (center == null || width == null) {
        vp.resetProperties();
      } else {
        vp.setProperties({
          voiRange: { lower: center - width / 2, upper: center + width / 2 },
        });
      }
      vp.render();
    } catch (e) {
      console.error("窗位设置失败", e);
    }
  };

  const handleReset = () => {
    const vp = getViewport();
    if (!vp) return;
    try {
      vp.resetCamera();
      vp.resetProperties();
      vp.render();
    } catch (e) {
      console.error("重置失败", e);
    }
  };

  return (
    <div
      className="flex flex-col h-full w-full"
      onClick={(e) => e.stopPropagation()}
    >
      <div className="relative z-10 flex items-center gap-2 px-3 py-2 shrink-0 flex-wrap bg-black/60">
        {/* 左键工具:窗宽窗位 / 长度测量。滚轮滚动切片无需选工具。 */}
        <button
          onClick={() => setLeftTool("WindowLevel")}
          className={`flex items-center gap-1.5 text-xs px-3 py-1.5 rounded-lg cursor-pointer transition-colors ${
            leftTool === "WindowLevel"
              ? "bg-white text-slate-900"
              : "bg-white/10 text-white/80 hover:bg-white/20"
          }`}
        >
          <SlidersHorizontal className="w-3.5 h-3.5" /> 窗宽窗位
        </button>
        <button
          onClick={() => setLeftTool("Length")}
          className={`flex items-center gap-1.5 text-xs px-3 py-1.5 rounded-lg cursor-pointer transition-colors ${
            leftTool === "Length"
              ? "bg-white text-slate-900"
              : "bg-white/10 text-white/80 hover:bg-white/20"
          }`}
        >
          <Ruler className="w-3.5 h-3.5" /> 长度测量
        </button>

        <span className="w-px h-4 bg-white/20 mx-1" />

        {/* 窗位预设 */}
        {PRESETS.map((p) => (
          <button
            key={p.label}
            onClick={() => applyPreset(p.center, p.width)}
            className="text-xs px-2.5 py-1.5 rounded-lg bg-white/10 text-white/80 hover:bg-white/20 cursor-pointer transition-colors"
          >
            {p.label}
          </button>
        ))}

        <span className="w-px h-4 bg-white/20 mx-1" />

        <button
          onClick={handleReset}
          className="flex items-center gap-1.5 text-xs px-3 py-1.5 rounded-lg bg-white/10 text-white/80 hover:bg-white/20 cursor-pointer"
        >
          <RotateCcw className="w-3.5 h-3.5" /> 重置
        </button>

        {sliceTotal > 1 && ready && (
          <span className="text-xs font-mono text-white/70 ml-1">
            第 {sliceIndex + 1} / 共 {sliceTotal} 张
          </span>
        )}
        {error && <span className="text-xs text-rose-300 ml-1">{error}</span>}
        {!ready && !error && (
          <span className="text-xs text-white/50 ml-1">加载中…</span>
        )}
        {ready && !error && (
          <span className="text-[11px] text-white/40 ml-auto hidden sm:inline">
            滚轮翻页 · 左键窗位 · 右键缩放 · 中键平移
          </span>
        )}
      </div>
      <div
        ref={elementRef}
        className="flex-1 min-h-0 relative bg-black overflow-hidden"
        style={{ touchAction: "none" }}
        onContextMenu={(e) => e.preventDefault()}
      />
    </div>
  );
}
