import 'dart:io';

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
    return OcrResult(recognized.text, _averageMlkitConfidence(recognized));
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
