import 'dart:io';
import 'dart:math' as math;

import 'package:flutter/services.dart';
import 'package:google_mlkit_text_recognition/google_mlkit_text_recognition.dart';

/// 一张图片的 OCR 结果:识别文本 + 平均置信度(0~1)。
class OcrResult {
  final String text;
  final double confidence;
  const OcrResult(this.text, this.confidence);
}

const MethodChannel _channel = MethodChannel('medme/ocr');

/// 置信度拿不到时的兜底值(空文本/引擎不给),让导入流程照常继续。
const double _confFallback = 0.9;

/// 识别一张图片里的文字。**各平台用原生最强引擎、功能一致**:
/// - iOS:Apple Vision(`VNRecognizeTextRequest`,中文更强,原生支持 HEIC),经
///   `medme/ocr` MethodChannel 调用(见 `ios/Runner/AppDelegate.swift`)。
/// - 安卓/其它:ML Kit 中文文本识别。
///
/// 返回 [OcrResult];引擎/路径异常时降级为空文本(上层据此走「仅存原件」),不抛。
Future<OcrResult> recognizeImageText(String path) async {
  if (Platform.isIOS) {
    try {
      final res = await _channel.invokeMapMethod<String, dynamic>('recognize', {
        'path': path,
      });
      final text = (res?['text'] as String?) ?? '';
      final conf = (res?['confidence'] as num?)?.toDouble() ?? _confFallback;
      return OcrResult(text, conf);
    } on PlatformException {
      return const OcrResult('', _confFallback);
    }
  }
  // 安卓 + 其它:ML Kit 中文
  final recognizer = TextRecognizer(script: TextRecognitionScript.chinese);
  try {
    final recognized = await recognizer.processImage(
      InputImage.fromFilePath(path),
    );
    // 化验单等表格文档,列信息全靠 OCR 各行的空间坐标——ML Kit 把同一物理行里
    // 相隔较远的几格识别成独立 TextLine,直接拼文本会丢空格、塌成流水账。
    // 按 boundingBox 重建:同一视觉行(y 接近)的多个 TextLine 按 x 坐标补空格
    // 对齐成列;单 TextLine 的行(散文句子)原样输出。无文字时回退 recognized.text
    // (逐字节等价旧行为)。
    final layoutText = _rebuildLayoutText(recognized);
    final text = layoutText.isNotEmpty ? layoutText : recognized.text;
    return OcrResult(text, _averageMlkitConfidence(recognized));
  } catch (_) {
    // OCR 失败(模型首次下载失败 / 坏图等)也返回空文本,与 iOS 对齐:让导入流程
    // 继续,原件照样存下(「仅存原件·未识别到文字」),不丢用户的照片。
    return const OcrResult('', _confFallback);
  } finally {
    await recognizer.close();
  }
}

/// ML Kit 结果的平均置信度:遍历 blocks→lines→elements 取有值的 `confidence` 求平均;
/// 全空(如无文本)回退到 [_confFallback]。
double _averageMlkitConfidence(RecognizedText recognized) {
  final values = <double>[];
  for (final block in recognized.blocks) {
    for (final line in block.lines) {
      for (final element in line.elements) {
        final c = element.confidence;
        if (c != null) values.add(c);
      }
    }
  }
  if (values.isEmpty) return _confFallback;
  return values.reduce((a, b) => a + b) / values.length;
}

/// OCR 列重建常量:目标列宽(字符数,映射整页文字内容宽度)、同行 y 容差
/// (相对行高的比例,判定多个 TextLine 是否属于同一视觉行)、段落间距阈值
/// (相对行高的比例,超过判定为新段/新块,输出空行)。保守取值,只在「同一
/// 视觉行确有多个文本片段」时才触发列对齐,规整散文不受影响。
const int _ocrTargetColumnWidth = 90;
const double _ocrRowYToleranceRatio = 0.6;
const double _ocrBlockGapRatio = 1.6;

/// 一个 ML Kit 识别行:文字 + 包围盒(像素坐标,原点左上)。
class _OcrLine {
  final String text;
  final Rect box;
  const _OcrLine(this.text, this.box);
}

/// 按 [TextLine] 的空间坐标重建版面文本:把所有块的行拍平、按阅读顺序排序,
/// 再按 y 坐标归到同一视觉行——视觉行只有一个 TextLine 时原样输出(散文);
/// 多个时按各自 x 坐标映射字符列、补空格重建对齐(表格的一行数据)。
/// 视觉行之间的纵向间距明显大于行高时插空行(段落/表格边界)。
String _rebuildLayoutText(RecognizedText recognized) {
  final allLines = <_OcrLine>[];
  for (final block in recognized.blocks) {
    for (final line in block.lines) {
      final text = line.text.trim();
      if (text.isNotEmpty) allLines.add(_OcrLine(text, line.boundingBox));
    }
  }
  if (allLines.isEmpty) return '';

  // 按阅读顺序排序(上→下、左→右),ML Kit 的 boundingBox 原点在左上。
  allLines.sort((a, b) {
    final byTop = a.box.top.compareTo(b.box.top);
    if (byTop != 0) return byTop;
    return a.box.left.compareTo(b.box.left);
  });

  // 1) 按 y 坐标把 TextLine 归到同一视觉行(容差 = 行高的一个比例)。
  final rows = <List<_OcrLine>>[];
  for (final line in allLines) {
    if (rows.isNotEmpty) {
      final refTop = rows.last.first.box.top;
      final tol =
          _ocrRowYToleranceRatio * math.max(line.box.height, rows.last.first.box.height);
      if ((line.box.top - refTop).abs() <= tol) {
        rows.last.add(line);
        continue;
      }
    }
    rows.add([line]);
  }

  // 2) 整页文字内容的左右边界,作为列坐标的归一化参照(而非整张图片像素宽度,
  //    避免照片背景空白挤压列分辨率;也不必额外解码图片拿宽高)。
  final contentLeft = allLines.map((l) => l.box.left).reduce(math.min);
  final contentRight = allLines.map((l) => l.box.right).reduce(math.max);
  final contentSpan = contentRight - contentLeft;

  // 3) 逐视觉行拼接,行距超过阈值(相对上一行行高)处插空行。
  final outLines = <String>[];
  double? prevTop;
  double? prevHeight;
  for (final row in rows) {
    final rowTop = row.map((l) => l.box.top).reduce(math.min);
    final rowHeight = row.map((l) => l.box.height).reduce(math.max);
    if (prevTop != null) {
      final refHeight = prevHeight ?? rowHeight;
      if (refHeight > 0 && rowTop - prevTop > _ocrBlockGapRatio * refHeight) {
        outLines.add(''); // 大纵向间隔 → 块边界(join 后成空行)
      }
    }
    outLines.add(_buildRowText(row, contentLeft, contentSpan));
    prevTop = rowTop;
    prevHeight = rowHeight;
  }
  return outLines.join('\n');
}

/// 把同一视觉行的 TextLine 拼成一行文本。单 TextLine 直接返回文字;多个
/// (表格的一行数据)按各自 left 在 `contentSpan` 里的相对位置映射到目标列宽的
/// 字符列,补空格对齐(至少 2 个空格,配合查看器 `splitCells` 的切列规则)。
String _buildRowText(List<_OcrLine> row, double contentLeft, double contentSpan) {
  final sortedRow = [...row]..sort((a, b) => a.box.left.compareTo(b.box.left));
  if (sortedRow.length <= 1 || contentSpan <= 0.0001) {
    return sortedRow.map((l) => l.text).join(' ');
  }
  final buf = StringBuffer();
  for (final line in sortedRow) {
    if (buf.isEmpty) {
      buf.write(line.text);
      continue;
    }
    final col = ((line.box.left - contentLeft) / contentSpan * _ocrTargetColumnWidth).round();
    final targetLen = math.max(col, buf.length + 2);
    buf.write(' ' * (targetLen - buf.length));
    buf.write(line.text);
  }
  return buf.toString();
}
