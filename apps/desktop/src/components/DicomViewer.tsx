import { useEffect, useRef, useState } from "react";
import dicomParser from "dicom-parser";
import { Stethoscope, RotateCcw, ZoomIn, ZoomOut } from "lucide-react";

// 轻量交互式 DICOM 查看器(纯 canvas + dicom-parser,无 Cornerstone / 无 web worker /
// 无 WASM)—— 之前的 Cornerstone3D 在 Tauri+Vite 下把整个应用白屏(JPEG codec 是
// CommonJS 无 default 导出、ESM/worker/WASM 与 Vite 打包冲突),故整体替换。
//
// 帧模型:每张切片(Uint8Array)用 dicomParser.parseDicom 解析成 dataSet;一个文件可能
// 是多帧(NumberOfFrames>1,如超声动态),展平成 frames[] 每项记 { dataSet, frameIndex }。
// 解码按需进行,只缓存当前帧 + 邻近几帧,几百帧也只常驻几十 MB(014 大数据顾虑)。
//
// 交互:滚轮翻层(主诉求)、左键拖拽调窗宽窗位(灰度)、右键拖拽缩放、中键拖拽平移、
// +/- 按钮缩放、窗位预设、重置。ESC 由外层 lightbox 处理。

const TS_UNCOMPRESSED = new Set([
  "1.2.840.10008.1.2", // Implicit VR Little Endian
  "1.2.840.10008.1.2.1", // Explicit VR Little Endian
  "1.2.840.10008.1.2.1.99", // Deflated Explicit VR LE(解析层已解压)
]);
const TS_JPEG_BASELINE = new Set([
  "1.2.840.10008.1.2.4.50", // JPEG Baseline (Process 1)
  "1.2.840.10008.1.2.4.51", // JPEG Extended (Process 2 & 4)
]);

// 窗宽窗位预设:center(C)/width(W)。默认 = 用 DICOM 自带窗位(无则用像素 min/max)。
const PRESETS: { label: string; center: number | null; width: number | null }[] = [
  { label: "默认", center: null, width: null },
  { label: "脑窗", center: 40, width: 80 },
  { label: "骨窗", center: 500, width: 2000 },
  { label: "肺窗", center: -600, width: 1500 },
  { label: "软组织", center: 40, width: 400 },
];

// 一帧的元信息(从 dataSet + 帧下标解出),解码时用。
interface FrameMeta {
  dataSet: dicomParser.DataSet;
  frameIndex: number;
  rows: number;
  columns: number;
  bitsAllocated: number;
  pixelRepresentation: number; // 0 无符号 / 1 有符号
  samplesPerPixel: number;
  planarConfiguration: number; // 0 交错 / 1 平面
  photometric: string;
  invert: boolean; // MONOCHROME1 → 反相
  color: boolean;
  transferSyntax: string;
  defaultCenter: number | null;
  defaultWidth: number | null;
}

// 解码结果缓存项。
type Decoded =
  | { kind: "gray"; values: Float32Array; rows: number; cols: number; invert: boolean }
  | { kind: "rgba"; data: ImageData }
  | { kind: "bitmap"; bitmap: ImageBitmap; rows: number; cols: number }
  | { kind: "unsupported" }
  | { kind: "error" };

const DECODE_CACHE_MAX = 7; // 当前帧 + 前后几帧;超出按离当前最远淘汰。

function num(ds: dicomParser.DataSet, tag: string, def: number): number {
  const v = ds.uint16(tag);
  return v === undefined ? def : v;
}
function floatStr(ds: dicomParser.DataSet, tag: string): number | null {
  const v = ds.floatString(tag);
  return v === undefined || Number.isNaN(v) ? null : v;
}

// 解析每张切片 → 展平成 frames[]。解析失败的切片跳过,不抛。
function buildFrames(slices: Uint8Array[]): FrameMeta[] {
  const frames: FrameMeta[] = [];
  for (const bytes of slices) {
    let ds: dicomParser.DataSet;
    try {
      ds = dicomParser.parseDicom(bytes);
    } catch {
      continue;
    }
    const rows = num(ds, "x00280010", 0);
    const columns = num(ds, "x00280011", 0);
    if (!rows || !columns) continue;
    const bitsAllocated = num(ds, "x00280100", 16);
    const pixelRepresentation = num(ds, "x00280103", 0);
    const samplesPerPixel = num(ds, "x00280002", 1);
    const planarConfiguration = num(ds, "x00280006", 0);
    const photometric = (ds.string("x00280004") || "MONOCHROME2").trim().toUpperCase();
    const transferSyntax = (ds.string("x00020010") || "1.2.840.10008.1.2").trim();
    const nFrames = parseInt(ds.string("x00280008") || "1", 10) || 1;
    const defaultCenter = floatStr(ds, "x00281050");
    const defaultWidth = floatStr(ds, "x00281051");
    for (let f = 0; f < nFrames; f++) {
      frames.push({
        dataSet: ds,
        frameIndex: f,
        rows,
        columns,
        bitsAllocated,
        pixelRepresentation,
        samplesPerPixel,
        planarConfiguration,
        photometric,
        invert: photometric === "MONOCHROME1",
        color: samplesPerPixel >= 3,
        transferSyntax,
        defaultCenter,
        defaultWidth,
      });
    }
  }
  return frames;
}

// 从对齐副本读取灰度原始像素 → 应用 modality rescale(v = raw*slope + intercept)。
function readGrayValues(fm: FrameMeta): Float32Array {
  const ds = fm.dataSet;
  const pd = ds.elements.x7fe00010;
  const byteArray = ds.byteArray;
  const bytesPerPixel = fm.bitsAllocated <= 8 ? 1 : 2;
  const pxCount = fm.rows * fm.columns;
  const frameLength = pxCount * bytesPerPixel; // SamplesPerPixel 1
  const absStart = byteArray.byteOffset + pd.dataOffset + fm.frameIndex * frameLength;
  // 复制到独立、对齐的 ArrayBuffer,保证 Int16/Uint16 视图的字节对齐正确。
  const buf = byteArray.buffer.slice(absStart, absStart + frameLength);
  const slope = floatStr(ds, "x00281053") ?? 1;
  const intercept = floatStr(ds, "x00281052") ?? 0;
  const out = new Float32Array(pxCount);
  if (bytesPerPixel === 1) {
    const raw = new Uint8Array(buf);
    for (let i = 0; i < pxCount; i++) out[i] = raw[i] * slope + intercept;
  } else if (fm.pixelRepresentation === 1) {
    const raw = new Int16Array(buf); // 小端(浏览器均小端)
    for (let i = 0; i < pxCount; i++) out[i] = raw[i] * slope + intercept;
  } else {
    const raw = new Uint16Array(buf);
    for (let i = 0; i < pxCount; i++) out[i] = raw[i] * slope + intercept;
  }
  return out;
}

// 未压缩彩色(RGB)→ ImageData。处理 PlanarConfiguration 0 交错 / 1 平面。
function readColor(fm: FrameMeta): ImageData {
  const ds = fm.dataSet;
  const pd = ds.elements.x7fe00010;
  const byteArray = ds.byteArray;
  const pxCount = fm.rows * fm.columns;
  const frameLength = pxCount * 3; // 8-bit RGB
  const absStart = byteArray.byteOffset + pd.dataOffset + fm.frameIndex * frameLength;
  const src = new Uint8Array(byteArray.buffer, absStart, frameLength);
  const img = new ImageData(fm.columns, fm.rows);
  const d = img.data;
  if (fm.planarConfiguration === 1) {
    const plane = pxCount;
    for (let i = 0; i < pxCount; i++) {
      d[i * 4] = src[i];
      d[i * 4 + 1] = src[plane + i];
      d[i * 4 + 2] = src[2 * plane + i];
      d[i * 4 + 3] = 255;
    }
  } else {
    for (let i = 0; i < pxCount; i++) {
      d[i * 4] = src[i * 3];
      d[i * 4 + 1] = src[i * 3 + 1];
      d[i * 4 + 2] = src[i * 3 + 2];
      d[i * 4 + 3] = 255;
    }
  }
  return img;
}

async function decodeFrame(fm: FrameMeta): Promise<Decoded> {
  try {
    const ts = fm.transferSyntax;
    if (TS_UNCOMPRESSED.has(ts)) {
      if (fm.color) return { kind: "rgba", data: readColor(fm) };
      return {
        kind: "gray",
        values: readGrayValues(fm),
        rows: fm.rows,
        cols: fm.columns,
        invert: fm.invert,
      };
    }
    if (TS_JPEG_BASELINE.has(ts)) {
      const pd = fm.dataSet.elements.x7fe00010;
      // BOT(基本偏移表)有条目时按帧读;很多设备的多帧(如超声动态)BOT 为空,此时
      // readEncapsulatedImageFrame 会抛错,改按 fragment 下标读(一帧一 fragment 的常见情形
      // fragmentIndex === frameIndex,能正确取出每帧 JPEG)。
      const bot = pd.basicOffsetTable;
      const encoded =
        bot && bot.length > 0
          ? dicomParser.readEncapsulatedImageFrame(fm.dataSet, pd, fm.frameIndex)
          : dicomParser.readEncapsulatedPixelDataFromFragments(fm.dataSet, pd, fm.frameIndex);
      const copy = encoded.slice(); // 独立 buffer 给 Blob
      const blob = new Blob([copy], { type: "image/jpeg" });
      const bitmap = await createImageBitmap(blob);
      return { kind: "bitmap", bitmap, rows: bitmap.height, cols: bitmap.width };
    }
    // JPEG2000 / JPEG-LS / RLE 等:不在此轻量查看器解码。
    return { kind: "unsupported" };
  } catch (e) {
    console.error("DICOM 帧解码失败", e);
    return { kind: "error" };
  }
}

export default function DicomViewer({
  slices,
}: {
  slices: Uint8Array[];
  fileName: string;
}) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);

  const framesRef = useRef<FrameMeta[]>([]);
  const cacheRef = useRef<Map<number, Decoded>>(new Map());
  const drawTokenRef = useRef(0);
  const winRef = useRef({ center: 40, width: 400 }); // 当前窗宽窗位(灰度)
  const viewRef = useRef({ zoom: 1, panX: 0, panY: 0 });
  const idxRef = useRef(0);
  const dragRef = useRef<{ button: number; x: number; y: number } | null>(null);

  const [frameTotal, setFrameTotal] = useState(0);
  const [frameIndex, setFrameIndex] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [ready, setReady] = useState(false);
  // 首次打开中央浮一条"滚轮翻层"提示,几秒后淡出。
  const [showHint, setShowHint] = useState(true);

  // 把已解码帧画到 canvas(应用 fit + 缩放/平移;灰度再应用窗宽窗位)。
  const paint = (dec: Decoded) => {
    const canvas = canvasRef.current;
    const container = containerRef.current;
    if (!canvas || !container) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const cw = container.clientWidth;
    const ch = container.clientHeight;
    const dpr = window.devicePixelRatio || 1;
    if (canvas.width !== Math.round(cw * dpr) || canvas.height !== Math.round(ch * dpr)) {
      canvas.width = Math.round(cw * dpr);
      canvas.height = Math.round(ch * dpr);
      canvas.style.width = `${cw}px`;
      canvas.style.height = `${ch}px`;
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.fillStyle = "#000";
    ctx.fillRect(0, 0, cw, ch);

    if (dec.kind === "unsupported" || dec.kind === "error") return;

    // 组一张原尺寸 offscreen 图。
    let srcCols: number;
    let srcRows: number;
    let source: CanvasImageSource;
    if (dec.kind === "bitmap") {
      srcCols = dec.cols;
      srcRows = dec.rows;
      source = dec.bitmap;
    } else if (dec.kind === "rgba") {
      srcCols = dec.data.width;
      srcRows = dec.data.height;
      const off = document.createElement("canvas");
      off.width = srcCols;
      off.height = srcRows;
      off.getContext("2d")!.putImageData(dec.data, 0, 0);
      source = off;
    } else {
      // gray:按当前窗宽窗位映射到 8-bit 灰阶。
      srcCols = dec.cols;
      srcRows = dec.rows;
      const { center, width } = winRef.current;
      const w = width <= 0 ? 1 : width;
      const low = center - w / 2;
      const img = new ImageData(srcCols, srcRows);
      const d = img.data;
      const vals = dec.values;
      for (let i = 0; i < vals.length; i++) {
        let out = ((vals[i] - low) / w) * 255;
        out = out < 0 ? 0 : out > 255 ? 255 : out;
        if (dec.invert) out = 255 - out;
        const o = i * 4;
        d[o] = d[o + 1] = d[o + 2] = out;
        d[o + 3] = 255;
      }
      const off = document.createElement("canvas");
      off.width = srcCols;
      off.height = srcRows;
      off.getContext("2d")!.putImageData(img, 0, 0);
      source = off;
    }

    const fit = Math.min(cw / srcCols, ch / srcRows);
    const { zoom, panX, panY } = viewRef.current;
    const dw = srcCols * fit * zoom;
    const dh = srcRows * fit * zoom;
    const dx = (cw - dw) / 2 + panX;
    const dy = (ch - dh) / 2 + panY;
    ctx.imageSmoothingEnabled = true;
    ctx.drawImage(source, dx, dy, dw, dh);
  };

  // 计算某帧的"默认"窗宽窗位:标签优先,否则用灰度像素 min/max。
  const defaultWindowFor = (fm: FrameMeta, dec: Decoded): { center: number; width: number } => {
    if (fm.defaultCenter != null && fm.defaultWidth != null && fm.defaultWidth > 0) {
      return { center: fm.defaultCenter, width: fm.defaultWidth };
    }
    if (dec.kind === "gray") {
      let mn = Infinity;
      let mx = -Infinity;
      const v = dec.values;
      for (let i = 0; i < v.length; i++) {
        if (v[i] < mn) mn = v[i];
        if (v[i] > mx) mx = v[i];
      }
      if (!Number.isFinite(mn) || !Number.isFinite(mx) || mx <= mn) {
        return { center: 128, width: 256 };
      }
      return { center: (mn + mx) / 2, width: mx - mn };
    }
    return { center: 128, width: 256 };
  };

  // 淘汰离当前帧最远的缓存项(bitmap 需 close 释放)。
  const evictCache = (current: number) => {
    const cache = cacheRef.current;
    while (cache.size > DECODE_CACHE_MAX) {
      let far = -1;
      let farDist = -1;
      for (const k of cache.keys()) {
        const dist = Math.abs(k - current);
        if (dist > farDist) {
          farDist = dist;
          far = k;
        }
      }
      if (far < 0) break;
      const ent = cache.get(far);
      if (ent && ent.kind === "bitmap") ent.bitmap.close();
      cache.delete(far);
    }
  };

  // 显示第 idx 帧:命中缓存直接画,否则解码(异步,token 防竞态),顺带预取邻帧。
  const showFrame = async (idx: number, resetWindow = false) => {
    const frames = framesRef.current;
    if (idx < 0 || idx >= frames.length) return;
    idxRef.current = idx;
    setFrameIndex(idx);
    setNotice(null);
    const token = ++drawTokenRef.current;
    const fm = frames[idx];

    let dec = cacheRef.current.get(idx);
    if (!dec) {
      dec = await decodeFrame(fm);
      if (token !== drawTokenRef.current) {
        if (dec.kind === "bitmap") dec.bitmap.close();
        return;
      }
      cacheRef.current.set(idx, dec);
      evictCache(idx);
    }
    if (token !== drawTokenRef.current) return;

    if (dec.kind === "unsupported") {
      setNotice("此压缩格式暂不支持交互查看,请用上方原件缩略图");
    }
    if (dec.kind === "error") {
      setNotice("影像加载失败");
    }
    if (resetWindow) {
      winRef.current = defaultWindowFor(fm, dec);
    }
    paint(dec);
    prefetch(idx);
  };

  // 预取当前帧前后各一帧(不阻塞、不改动 token / 当前显示)。
  const prefetch = (idx: number) => {
    const frames = framesRef.current;
    for (const j of [idx + 1, idx - 1]) {
      if (j < 0 || j >= frames.length || cacheRef.current.has(j)) continue;
      decodeFrame(frames[j]).then((d) => {
        if (cacheRef.current.has(j)) {
          if (d.kind === "bitmap") d.bitmap.close();
          return;
        }
        cacheRef.current.set(j, d);
        evictCache(idxRef.current);
      });
    }
  };

  // 只重画当前帧(窗位 / 缩放 / 平移变化后,无需重新解码)。
  const redraw = () => {
    const dec = cacheRef.current.get(idxRef.current);
    if (dec) paint(dec);
  };

  // 解析切片 → 建 frames → 显示第 0 帧。所有解析/解码包在 try/catch,绝不冒泡到 React。
  useEffect(() => {
    let disposed = false;
    setReady(false);
    setError(null);
    setNotice(null);
    setFrameIndex(0);
    cacheRef.current.clear();
    viewRef.current = { zoom: 1, panX: 0, panY: 0 };
    try {
      const frames = buildFrames(slices);
      framesRef.current = frames;
      setFrameTotal(frames.length);
      if (frames.length === 0) {
        setError("影像加载失败");
        return;
      }
      idxRef.current = 0;
      showFrame(0, true).then(() => {
        if (!disposed) setReady(true);
      });
    } catch (e) {
      console.error("DICOM 加载失败", e);
      setError("影像加载失败");
    }
    return () => {
      disposed = true;
      drawTokenRef.current++;
      for (const ent of cacheRef.current.values()) {
        if (ent.kind === "bitmap") ent.bitmap.close();
      }
      cacheRef.current.clear();
      framesRef.current = [];
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [slices]);

  // 容器尺寸变化 → 重画。
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const ro = new ResizeObserver(() => redraw());
    ro.observe(container);
    return () => ro.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 加载完成后显示"滚轮翻层"提示,4.5 秒后淡出。
  useEffect(() => {
    if (!ready) return;
    setShowHint(true);
    const t = setTimeout(() => setShowHint(false), 4500);
    return () => clearTimeout(t);
  }, [ready]);
  useEffect(() => {
    if (frameIndex > 0) setShowHint(false);
  }, [frameIndex]);

  // 滚轮:Ctrl → 缩放;否则翻层(主诉求)。React 的 onWheel 是被动监听,preventDefault
  // 无效,故用原生 { passive:false } 监听。
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      if (e.ctrlKey) {
        const factor = e.deltaY > 0 ? 0.9 : 1.1;
        viewRef.current.zoom = Math.min(20, Math.max(0.2, viewRef.current.zoom * factor));
        redraw();
        return;
      }
      setShowHint(false);
      const next = idxRef.current + (e.deltaY > 0 ? 1 : -1);
      if (next < 0 || next >= framesRef.current.length) return;
      showFrame(next);
    };
    canvas.addEventListener("wheel", onWheel, { passive: false });
    return () => canvas.removeEventListener("wheel", onWheel);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const onMouseDown = (e: React.MouseEvent) => {
    dragRef.current = { button: e.button, x: e.clientX, y: e.clientY };
  };
  const onMouseMove = (e: React.MouseEvent) => {
    const drag = dragRef.current;
    if (!drag) return;
    const dx = e.clientX - drag.x;
    const dy = e.clientY - drag.y;
    drag.x = e.clientX;
    drag.y = e.clientY;
    const dec = cacheRef.current.get(idxRef.current);
    if (drag.button === 0) {
      // 左键:灰度调窗宽窗位(dx→宽,dy→中心);彩色帧无意义则平移。
      if (dec && dec.kind === "gray") {
        winRef.current.width = Math.max(1, winRef.current.width + dx * 2);
        winRef.current.center = winRef.current.center + dy * 2;
        redraw();
      } else {
        viewRef.current.panX += dx;
        viewRef.current.panY += dy;
        redraw();
      }
    } else if (drag.button === 2) {
      // 右键:缩放(上移放大)。
      const factor = Math.exp(-dy * 0.005);
      viewRef.current.zoom = Math.min(20, Math.max(0.2, viewRef.current.zoom * factor));
      redraw();
    } else {
      // 中键:平移。
      viewRef.current.panX += dx;
      viewRef.current.panY += dy;
      redraw();
    }
  };
  const onMouseUp = () => {
    dragRef.current = null;
  };

  const applyPreset = (center: number | null, width: number | null) => {
    const fm = framesRef.current[idxRef.current];
    const dec = cacheRef.current.get(idxRef.current);
    if (!fm || !dec) return;
    if (center == null || width == null) {
      winRef.current = defaultWindowFor(fm, dec);
    } else {
      winRef.current = { center, width };
    }
    redraw();
  };

  const zoomBy = (factor: number) => {
    viewRef.current.zoom = Math.min(20, Math.max(0.2, viewRef.current.zoom * factor));
    redraw();
  };

  const handleReset = () => {
    const fm = framesRef.current[idxRef.current];
    const dec = cacheRef.current.get(idxRef.current);
    viewRef.current = { zoom: 1, panX: 0, panY: 0 };
    if (fm && dec) winRef.current = defaultWindowFor(fm, dec);
    redraw();
  };

  return (
    <div className="flex flex-col h-full w-full" onClick={(e) => e.stopPropagation()}>
      <div className="relative z-10 flex items-center gap-2 px-3 py-2 shrink-0 flex-wrap bg-black/60">
        {/* 缩放 */}
        <button
          onClick={() => zoomBy(1.2)}
          className="flex items-center gap-1.5 text-xs px-3 py-1.5 rounded-lg bg-white/10 text-white/80 hover:bg-white/20 cursor-pointer transition-colors"
        >
          <ZoomIn className="w-3.5 h-3.5" /> 放大
        </button>
        <button
          onClick={() => zoomBy(1 / 1.2)}
          className="flex items-center gap-1.5 text-xs px-3 py-1.5 rounded-lg bg-white/10 text-white/80 hover:bg-white/20 cursor-pointer transition-colors"
        >
          <ZoomOut className="w-3.5 h-3.5" /> 缩小
        </button>

        <span className="w-px h-4 bg-white/20 mx-1" />

        {/* 专业阅片工具:窗宽窗位预设(给医生看片调明暗;普通用户滚轮看图即可,可无视这组)*/}
        <span
          className="flex items-center gap-1 text-[11px] font-medium text-amber-300"
          title="窗宽窗位是给医生调节明暗看不同组织的专业工具"
        >
          <Stethoscope className="w-3.5 h-3.5" /> 医生 · 窗位
        </span>
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

        {frameTotal > 1 && ready && (
          <span className="text-xs font-mono text-white/70 ml-1">
            第 {frameIndex + 1} / 共 {frameTotal} 张
          </span>
        )}
        {error && <span className="text-xs text-rose-300 ml-1">{error}</span>}
        {!ready && !error && <span className="text-xs text-white/50 ml-1">加载中…</span>}
        {ready && !error && (
          <span className="ml-auto text-[11px] text-white/70">
            {frameTotal > 1 ? "↕ 滚轮翻看每一层 · " : ""}左键调明暗 · 右键缩放 · ESC 返回
          </span>
        )}
      </div>
      <div className="flex-1 min-h-0 relative">
        <div
          ref={containerRef}
          className="absolute inset-0 bg-black overflow-hidden"
          style={{ touchAction: "none" }}
        >
          <canvas
            ref={canvasRef}
            className="block"
            style={{ touchAction: "none" }}
            onMouseDown={onMouseDown}
            onMouseMove={onMouseMove}
            onMouseUp={onMouseUp}
            onMouseLeave={onMouseUp}
            onContextMenu={(e) => e.preventDefault()}
          />
        </div>
        {notice && (
          <div className="pointer-events-none absolute top-4 left-1/2 -translate-x-1/2 bg-black/75 text-white/90 text-xs px-4 py-2 rounded-lg max-w-[80%] text-center">
            {notice}
          </div>
        )}
        {ready && showHint && frameTotal > 1 && (
          <div className="pointer-events-none absolute bottom-6 left-1/2 -translate-x-1/2 bg-black/75 text-white text-sm px-5 py-2.5 rounded-full flex items-center gap-2 shadow-lg">
            <span className="text-base">↕</span> 滚轮上下翻看每一层(共 {frameTotal} 张)
          </div>
        )}
      </div>
    </div>
  );
}
