import { useEffect, useRef, useState } from "react";
import { App, AppOptions, ToolConfig, ViewConfig } from "dwv";
import { SlidersHorizontal, Move, Layers, RotateCcw } from "lucide-react";

// 交互式 DICOM 查看器(基于 dwv):窗宽窗位 / 缩放平移 / 序列滚动(多帧)。
// 仅在全屏 lightbox 中挂载,卸载时清理 dwv App 实例与事件监听,避免泄漏。

type ToolId = "WindowLevel" | "ZoomAndPan" | "Scroll";

const TOOLS: { id: ToolId; label: string; icon: typeof SlidersHorizontal }[] = [
  { id: "WindowLevel", label: "窗宽窗位", icon: SlidersHorizontal },
  { id: "ZoomAndPan", label: "缩放平移", icon: Move },
  { id: "Scroll", label: "序列滚动", icon: Layers },
];

let seq = 0;

export default function DicomViewer({
  bytes,
  fileName,
}: {
  bytes: Uint8Array;
  fileName: string;
}) {
  const containerId = useRef(`dwv-layer-group-${++seq}`).current;
  const appRef = useRef<App | null>(null);
  const [tool, setTool] = useState<ToolId>("WindowLevel");
  const [ready, setReady] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // 初始化/加载:bytes 或 fileName 变化时重建(理论上每次挂载只发生一次)。
  // dwv 的 init/loadImageObject 对畸形 DICOM 可能同步抛错 —— 用 try/catch 转成内联
  // 错误提示,而不是让异常冒泡到 React 渲染阶段把整个应用白屏。
  useEffect(() => {
    const app = new App();
    appRef.current = app;
    setReady(false);
    setError(null);

    const handleLoad = () => {
      setReady(true);
      app.setTool("WindowLevel");
    };
    const handleError = (event: unknown) => {
      console.error("DICOM 加载失败", event);
      setError("DICOM 加载失败");
    };
    const handleResize = () => app.onResize();

    try {
      const viewConfig0 = new ViewConfig(containerId);
      const options = new AppOptions({ "*": [viewConfig0] });
      options.tools = {
        WindowLevel: new ToolConfig(),
        ZoomAndPan: new ToolConfig(),
        Scroll: new ToolConfig(),
      };
      app.init(options);

      app.addEventListener("load", handleLoad);
      app.addEventListener("error", handleError);
      window.addEventListener("resize", handleResize);

      // dwv 的内存加载接口需要独立的 ArrayBuffer。
      const buffer = bytes.buffer.slice(
        bytes.byteOffset,
        bytes.byteOffset + bytes.byteLength
      ) as ArrayBuffer;
      app.loadImageObject([{ name: fileName, filename: fileName, data: buffer }]);
    } catch (e) {
      console.error("DICOM 加载失败", e);
      setError("DICOM 加载失败");
    }

    return () => {
      window.removeEventListener("resize", handleResize);
      app.removeEventListener("load", handleLoad);
      app.removeEventListener("error", handleError);
      try {
        app.reset();
      } catch {
        /* 已处于错误状态时 reset 可能抛错,忽略 */
      }
      appRef.current = null;
    };
  }, [bytes, fileName, containerId]);

  // 工具切换。
  useEffect(() => {
    if (ready) appRef.current?.setTool(tool);
  }, [tool, ready]);

  const handleReset = () => {
    appRef.current?.resetZoomPan();
    appRef.current?.resetViews();
  };

  return (
    <div
      className="flex flex-col h-full w-full"
      onClick={(e) => e.stopPropagation()}
    >
      <div className="relative z-10 flex items-center gap-2 px-3 py-2 shrink-0 flex-wrap bg-black/60">
        {TOOLS.map(({ id, label, icon: Icon }) => (
          <button
            key={id}
            onClick={() => setTool(id)}
            className={`flex items-center gap-1.5 text-xs px-3 py-1.5 rounded-lg cursor-pointer transition-colors ${
              tool === id
                ? "bg-white text-slate-900"
                : "bg-white/10 text-white/80 hover:bg-white/20"
            }`}
          >
            <Icon className="w-3.5 h-3.5" /> {label}
          </button>
        ))}
        <button
          onClick={handleReset}
          className="flex items-center gap-1.5 text-xs px-3 py-1.5 rounded-lg bg-white/10 text-white/80 hover:bg-white/20 cursor-pointer"
        >
          <RotateCcw className="w-3.5 h-3.5" /> 重置
        </button>
        {error && <span className="text-xs text-rose-300 ml-1">{error}</span>}
        {!ready && !error && (
          <span className="text-xs text-white/50 ml-1">加载中…</span>
        )}
      </div>
      <div
        id={containerId}
        className="flex-1 min-h-0 relative bg-black overflow-hidden"
        style={{ touchAction: "none" }}
      />
    </div>
  );
}
