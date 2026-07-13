import 'dart:io';

import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:google_mlkit_text_recognition/google_mlkit_text_recognition.dart';
import 'package:image_picker/image_picker.dart';

import 'package:mobile_flutter/screens/import_helpers.dart';
import 'package:mobile_flutter/src/rust/api/dto.dart';
import 'package:mobile_flutter/src/rust/api/vault.dart';
import 'package:mobile_flutter/theme.dart';
import 'package:mobile_flutter/vault_events.dart';
import 'package:mobile_flutter/review_state.dart';

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
      final file = await ImagePicker().pickImage(source: ImageSource.camera);
      if (file == null) return const [];
      return [PendingImport(name: file.name, path: file.path, isImage: true)];
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

  TextRecognizer? recognizer;
  final rows = <ImportResultRow>[];
  final newDocIds = <int>[]; // 本次新建的文档 id → 加入「待确认」队列
  for (var i = 0; i < items.length; i++) {
    final item = items[i];
    progress.value = '正在导入 ${i + 1}/${items.length}…';
    try {
      final ImportOutcomeDto outcome;
      if (item.isImage) {
        recognizer ??= TextRecognizer(script: TextRecognitionScript.chinese);
        final recognized = await recognizer.processImage(
          InputImage.fromFilePath(item.path),
        );
        final bytes = await File(item.path).readAsBytes();
        outcome = await ingestImageWithText(
          name: item.name,
          bytes: bytes,
          ocrText: recognized.text,
          confidence: averageMlkitConfidence(recognized),
        );
      } else {
        final bytes = await File(item.path).readAsBytes();
        outcome = await ingestBytes(filename: item.name, data: bytes);
      }
      if (outcome.documentId case final id?) newDocIds.add(id);
      rows.add(rowFromOutcome(outcome));
    } catch (e) {
      rows.add(rowFromError(item.name, e));
    }
  }
  await recognizer?.close();

  // 本次新建的文档显式加入「待确认」队列(健康档案顶部据此置顶让用户核对)。
  if (newDocIds.isNotEmpty) {
    await ReviewState.instance.markPending(newDocIds);
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
