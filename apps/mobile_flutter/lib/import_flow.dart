import 'dart:io';

import 'package:cunning_document_scanner/cunning_document_scanner.dart';
import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:image_picker/image_picker.dart';
import 'package:pdfx/pdfx.dart';

import 'package:mobile_flutter/ocr_bridge.dart';
import 'package:mobile_flutter/screens/import_helpers.dart';
import 'package:mobile_flutter/src/rust/api/dto.dart';
import 'package:mobile_flutter/src/rust/api/vault.dart';
import 'package:mobile_flutter/theme.dart';
import 'package:mobile_flutter/vault_events.dart';
import 'package:mobile_flutter/review_state.dart';
import 'package:mobile_flutter/vault_boot.dart';

/// 「健康档案」右上角「+ 导入」触发的采集流程:弹三选一(拍照 / 相册 / 选文件),
/// 选定后逐个采集→(图片先 ML Kit 中文 OCR)→落库,期间显示进度对话框,结束弹汇总,
/// 并 [bumpVaultRevision] 通知档案自动刷新看到新记录。
///
/// 采集/OCR/落库逻辑与原「导入导出」屏一致,只是进度改用模态对话框(从档案触发,
/// 不再挂在某个屏的持久状态上)。医疗判断全在 Rust core,这里只搬字节 + 调 FFI。
Future<void> showImportSheet(BuildContext context) async {
  final choice = await showModalBottomSheet<_ImportChoice>(
    context: context,
    showDragHandle: true,
    builder: (context) => SafeArea(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          const Padding(
            padding: EdgeInsets.fromLTRB(20, 4, 20, 8),
            child: Align(
              alignment: Alignment.centerLeft,
              child: Text(
                '添加病历',
                style: TextStyle(fontSize: 16, fontWeight: FontWeight.w700),
              ),
            ),
          ),
          _SheetTile(
            icon: Icons.photo_camera_outlined,
            title: '拍照',
            subtitle: '对着化验单、处方拍一张,自动识别上面的文字',
            choice: _ImportChoice.camera,
          ),
          _SheetTile(
            icon: Icons.photo_library_outlined,
            title: '从相册选',
            subtitle: '选一张或多张已经拍好的病历照片',
            choice: _ImportChoice.gallery,
          ),
          _SheetTile(
            icon: Icons.folder_open_outlined,
            title: '选择文件',
            subtitle: 'PDF、图片、TXT',
            choice: _ImportChoice.files,
          ),
          const SizedBox(height: 8),
        ],
      ),
    ),
  );
  if (choice == null || !context.mounted) return;

  final items = await _pick(choice);
  if (items.isEmpty || !context.mounted) return;
  await _runImport(context, items);
}

enum _ImportChoice { camera, gallery, files }

Future<List<PendingImport>> _pick(_ImportChoice choice) async {
  switch (choice) {
    case _ImportChoice.camera:
      // 走系统文档扫描器(iOS VisionKit / 安卓 ML Kit Document Scanner):自动
      // 画框 + 透视校正,拿到已拉正的图 —— 斜着拍的表格变回横平竖直,OCR 才拼得
      // 回整行。随手斜拍是最常见的输入,这一步质量提升值一次多余的对框操作。
      // 扫描器不可用(部分设备/权限)时回退到普通拍照,不阻断采集。
      try {
        final paths = await CunningDocumentScanner.getPictures(
          scannerSource: ScannerSource.camera,
        );
        if (paths == null || paths.isEmpty) return const [];
        return [
          for (final p in paths)
            PendingImport(name: p.split('/').last, path: p, isImage: true),
        ];
      } catch (e) {
        debugPrint('[import] 文档扫描器不可用,回退普通拍照: $e');
        final file = await ImagePicker().pickImage(source: ImageSource.camera);
        if (file == null) return const [];
        return [PendingImport(name: file.name, path: file.path, isImage: true)];
      }
    case _ImportChoice.gallery:
      final files = await ImagePicker().pickMultiImage();
      return [
        for (final f in files)
          PendingImport(name: f.name, path: f.path, isImage: true),
      ];
    case _ImportChoice.files:
      final result = await FilePicker.platform.pickFiles(
        allowMultiple: true,
        type: FileType.custom,
        allowedExtensions: ['pdf', 'txt', 'png', 'jpg', 'jpeg', 'tiff', 'heic'],
      );
      if (result == null) return const [];
      return [
        for (final f in result.files)
          if (f.path != null)
            PendingImport(
              name: f.name,
              path: f.path!,
              isImage: isImageName(f.name),
            ),
      ];
  }
}

Future<void> _runImport(BuildContext context, List<PendingImport> items) async {
  final progress = ValueNotifier<String>('正在导入 1/${items.length}…');
  // 模态进度对话框(不可点走);导入结束后由本函数关闭。
  showDialog<void>(
    context: context,
    barrierDismissible: false,
    builder: (context) => AlertDialog(
      content: Row(
        children: [
          const SizedBox(
            width: 22,
            height: 22,
            child: CircularProgressIndicator(strokeWidth: 2.5),
          ),
          const SizedBox(width: 16),
          Expanded(
            child: ValueListenableBuilder<String>(
              valueListenable: progress,
              builder: (context, text, _) => Text(text),
            ),
          ),
        ],
      ),
    ),
  );

  final rows = <ImportResultRow>[];
  // 本次新建文档 id → 报告里识别到的患者姓名(识别不到为 null),进「待确认」队列;
  // 姓名与当前成员不符者会被标红,识别到的姓名还用来自动命名默认档案。
  final newDocs = <int, String?>{};
  for (var i = 0; i < items.length; i++) {
    final item = items[i];
    progress.value = '正在导入 ${i + 1}/${items.length}…';
    try {
      final ImportOutcomeDto outcome;
      var pdfBackfilled = false;
      if (item.isImage) {
        // 各平台原生最强 OCR:iOS Apple Vision / 安卓 ML Kit(见 ocr_bridge.dart)。
        final ocr = await recognizeImageText(item.path);
        final bytes = await File(item.path).readAsBytes();
        outcome = await ingestImageWithText(
          name: item.name,
          bytes: bytes,
          ocrText: ocr.text,
          confidence: ocr.confidence,
        );
      } else {
        final bytes = await File(item.path).readAsBytes();
        outcome = await ingestBytes(filename: item.name, data: bytes);
        // 扫描版 PDF(无文本层 → 仅存原件):移动端未链接 Rust OCR 引擎,改用 pdfx
        // 逐页渲染成 PNG、走能用的原生图片 OCR(Vision/ML Kit)后回填,补齐文本。
        if (outcome.status == 'stored_no_text' &&
            outcome.documentId != null &&
            item.name.toLowerCase().endsWith('.pdf')) {
          final pdfOcr = await _ocrScannedPdf(item.path);
          if (pdfOcr.text.trim().isNotEmpty) {
            await backfillPdfText(
              documentId: outcome.documentId!,
              text: pdfOcr.text,
              confidence: pdfOcr.confidence,
            );
            pdfBackfilled = true;
          }
        }
      }
      if (outcome.documentId case final id?) newDocs[id] = outcome.detectedName;
      rows.add(
        pdfBackfilled
            ? ImportResultRow(
                name: outcome.name,
                statusLabel: '已识别入库(扫描件)',
                kind: ImportRowKind.success,
              )
            : rowFromOutcome(outcome),
      );
    } catch (e) {
      // 原始错误留日志给开发者;用户看到的是 rowFromError 里的简单提示。
      debugPrint('[import] ${item.name} 导入失败: $e');
      rows.add(rowFromError(item.name, e));
    }
  }

  // 本次新建的文档显式加入「待确认」队列(健康档案顶部据此置顶让用户核对)。
  if (newDocs.isNotEmpty) {
    // 默认档案还没定过名字时,用识别到的第一个患者姓名自动命名它(迁移待确认键)。
    final detected = newDocs.values.firstWhere(
      (n) => n != null && n.trim().isNotEmpty,
      orElse: () => null,
    );
    await autoNameCurrentProfileFrom(detected);
    await ReviewState.instance.markPending(newDocs);
  }
  // 有任一份成功落库,通知「健康档案」屏自动刷新。
  if (rows.any((r) => r.kind != ImportRowKind.failed)) {
    bumpVaultRevision();
  }

  if (!context.mounted) return;
  Navigator.of(context).pop(); // 关进度对话框
  await _showImportSummary(context, rows);
  progress.dispose();
}

/// 扫描版 PDF 补 OCR:用 `pdfx` 逐页渲染成 PNG,走原生图片 OCR
/// ([recognizeImageText],iOS Vision / 安卓 ML Kit),合并各页文本 + 平均置信度。
/// 任何一步失败/无文本都安全返回(空文本 → 调用方不回填,保持「仅存原件」)。
/// 页数封顶 [_kMaxPdfOcrPages] 防超大 PDF 卡死。
const int _kMaxPdfOcrPages = 20;

/// 渲染放大倍数的候选,从清晰到保守**逐级降档**。
///
/// 分辨率越高 OCR 越准(笔画密的字在低解析度下会糊成一团),但设备内存有限:
/// 放大到一定程度 `render()` 会直接失败返回 null。此前写死单一倍数,一旦这台
/// 设备渲不出来就整篇零文本 —— 真机上就是这么炸的。所以不再赌一个常量,
/// 而是从高往低试,第一个渲得出来的就用。
///
/// 注意 pdfx 的 `page.width/height` 文档写明是 **像素**(不是 point),所以这里
/// 是相对原始尺寸的倍数,不能按 DPI 直接换算。
const List<double> _kRenderScales = [3.0, 2.0, 1.5];

Future<OcrResult> _ocrScannedPdf(String path) async {
  final buf = StringBuffer();
  final confs = <double>[];
  PdfDocument? doc;
  Directory? tmp;
  try {
    doc = await PdfDocument.openFile(path);
    tmp = await Directory.systemTemp.createTemp('medme_pdf_ocr');
    final pages = doc.pagesCount < _kMaxPdfOcrPages
        ? doc.pagesCount
        : _kMaxPdfOcrPages;
    for (var i = 1; i <= pages; i++) {
      final page = await doc.getPage(i);
      try {
        // 逐级降档:清晰优先,渲不出来就退一档,别让整页变成零文本。
        PdfPageImage? img;
        for (final scale in _kRenderScales) {
          try {
            img = await page.render(
              width: page.width * scale,
              height: page.height * scale,
              format: PdfPageImageFormat.png,
              // 必须给白底。PNG 默认透明背景,黑字压在透明底上被 OCR 加载时
              // 可能合成成黑底,变成黑字黑底、一个字都认不出。
              backgroundColor: '#FFFFFF',
            );
          } catch (e) {
            debugPrint('[import] 第 \$i 页 \$scale× 渲染异常: \$e');
          }
          if (img != null) {
            debugPrint('[import] 第 \$i 页以 \$scale× 渲染成功');
            break;
          }
          debugPrint('[import] 第 \$i 页 \$scale× 渲染失败,降档重试');
        }
        if (img == null) {
          debugPrint('[import] 第 $i 页所有倍数均渲染失败,跳过');
          continue;
        }
        final f = File('${tmp.path}/p$i.png');
        await f.writeAsBytes(img.bytes);
        final ocr = await recognizeImageText(f.path);
        if (ocr.text.trim().isNotEmpty) {
          // 页间必须空行分隔:OCR 已用 `\n\n` 分块(Layer-0),若这里只写一个
          // 换行,上一页的末块会和下一页的首块粘成同一块,下游按段分块就错位。
          if (buf.isNotEmpty) buf.write('\n\n');
          buf.write(ocr.text.trim());
          confs.add(ocr.confidence);
        }
      } finally {
        await page.close();
      }
    }
  } catch (e) {
    debugPrint('[import] 扫描 PDF 渲染/OCR 失败: $e');
  } finally {
    await doc?.close();
    if (tmp != null) {
      try {
        await tmp.delete(recursive: true);
      } catch (_) {}
    }
  }
  final conf = confs.isEmpty
      ? 0.0
      : confs.reduce((a, b) => a + b) / confs.length;
  return OcrResult(buf.toString().trim(), conf);
}

Future<void> _showImportSummary(
  BuildContext context,
  List<ImportResultRow> rows,
) async {
  final success = rows.where((r) => r.kind == ImportRowKind.success).length;
  final duplicate = rows.where((r) => r.kind == ImportRowKind.duplicate).length;
  final storedNoText = rows
      .where((r) => r.kind == ImportRowKind.storedNoText)
      .length;
  final failed = rows.where((r) => r.kind == ImportRowKind.failed).length;

  if (!context.mounted) return;
  await showDialog<void>(
    context: context,
    builder: (context) => AlertDialog(
      title: Text(failed == rows.length ? '导入未成功' : '导入完成'),
      content: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            if (success > 0)
              _summaryLine(Icons.check_circle, MedMe.teal, '成功识别入库 $success 份'),
            if (duplicate > 0)
              _summaryLine(
                Icons.content_copy,
                MedMe.faint,
                '重复,已跳过 $duplicate 份',
              ),
            if (storedNoText > 0)
              _summaryLine(
                Icons.warning_amber_rounded,
                Colors.orange,
                '仅存原件(未识别到文字)$storedNoText 份',
              ),
            if (failed > 0)
              _summaryLine(Icons.error_outline, MedMe.danger, '未能处理 $failed 份'),
            const SizedBox(height: 12),
            const Divider(height: 1, color: MedMe.line),
            const SizedBox(height: 8),
            for (final row in rows)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 3),
                child: Text(
                  '${row.name} —— ${row.statusLabel}',
                  style: const TextStyle(fontSize: 12.5, color: MedMe.faint),
                ),
              ),
          ],
        ),
      ),
      actions: [
        FilledButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('知道了'),
        ),
      ],
    ),
  );
}

Widget _summaryLine(IconData icon, Color color, String text) => Padding(
  padding: const EdgeInsets.symmetric(vertical: 4),
  child: Row(
    children: [
      Icon(icon, color: color, size: 20),
      const SizedBox(width: 8),
      Expanded(
        child: Text(
          text,
          style: const TextStyle(fontSize: 15, fontWeight: FontWeight.w600),
        ),
      ),
    ],
  ),
);

class _SheetTile extends StatelessWidget {
  const _SheetTile({
    required this.icon,
    required this.title,
    required this.subtitle,
    required this.choice,
  });

  final IconData icon;
  final String title;
  final String subtitle;
  final _ImportChoice choice;

  @override
  Widget build(BuildContext context) {
    return ListTile(
      leading: Icon(icon, color: MedMe.teal, size: 28),
      title: Text(title, style: const TextStyle(fontWeight: FontWeight.w600)),
      subtitle: Text(subtitle, style: const TextStyle(color: MedMe.faint)),
      onTap: () => Navigator.of(context).pop(choice),
    );
  }
}
