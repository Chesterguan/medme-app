import 'dart:io';

import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart' show Clipboard, ClipboardData;
import 'package:google_mlkit_text_recognition/google_mlkit_text_recognition.dart';
import 'package:image_picker/image_picker.dart';
import 'package:share_plus/share_plus.dart';

import 'package:mobile_flutter/screens/import_helpers.dart';
import 'package:mobile_flutter/src/rust/api/dto.dart';
import 'package:mobile_flutter/src/rust/api/vault.dart';
import 'package:mobile_flutter/theme.dart';

/// 底部导航一级 tab「导入导出」——采集(拍照 / 相册 / 选文件)+
/// 导出(时间线 HTML)/ 加密分享给医生。保险箱在 `main.dart` 启动时已打开,
/// 这里直接调 FFI;不重写任何医疗判断逻辑,识别/归类/加密全部落在 Rust core。
class ImportExportScreen extends StatefulWidget {
  const ImportExportScreen({super.key});

  @override
  State<ImportExportScreen> createState() => _ImportExportScreenState();
}

class _ImportExportScreenState extends State<ImportExportScreen> {
  bool _busy = false;
  String? _progress;

  // ---------------------------------------------------------------------
  // 采集入口:拍照 / 相册 / 文件选择器,三者最终都汇入 _importItems。
  // ---------------------------------------------------------------------

  Future<void> _pickFromCamera() async {
    final XFile? file = await ImagePicker().pickImage(
      source: ImageSource.camera,
    );
    if (file == null) return; // 用户取消
    await _importItems([
      PendingImport(name: file.name, path: file.path, isImage: true),
    ]);
  }

  Future<void> _pickFromGallery() async {
    final files = await ImagePicker().pickMultiImage();
    if (files.isEmpty) return; // 用户取消
    await _importItems([
      for (final f in files)
        PendingImport(name: f.name, path: f.path, isImage: true),
    ]);
  }

  Future<void> _pickFiles() async {
    final result = await FilePicker.platform.pickFiles(
      allowMultiple: true,
      type: FileType.custom,
      allowedExtensions: ['pdf', 'txt', 'png', 'jpg', 'jpeg', 'tiff', 'heic'],
    );
    if (result == null) return; // 用户取消
    final items = <PendingImport>[
      for (final f in result.files)
        if (f.path != null)
          PendingImport(
            name: f.name,
            path: f.path!,
            isImage: isImageName(f.name),
          ),
    ];
    if (items.isEmpty) return;
    await _importItems(items);
  }

  /// 逐个采集 → 落库,期间刷新「正在导入 i/n…」进度;结束后弹汇总对话框。
  /// 图片先用 ML Kit 中文 OCR 识别文字,再连同字节交给 `ingestImageWithText`;
  /// PDF/TXT 直传字节给 `ingestBytes`(按原始文件名后缀判 MIME)。单份文件
  /// 处理失败不影响其余文件继续导入。
  Future<void> _importItems(List<PendingImport> items) async {
    setState(() {
      _busy = true;
      _progress = '正在导入 1/${items.length}…';
    });

    TextRecognizer? recognizer;
    final rows = <ImportResultRow>[];
    for (var i = 0; i < items.length; i++) {
      final item = items[i];
      if (i > 0) {
        setState(() => _progress = '正在导入 ${i + 1}/${items.length}…');
      }
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
        rows.add(rowFromOutcome(outcome));
      } catch (e) {
        rows.add(rowFromError(item.name, e));
      }
    }
    await recognizer?.close();

    if (!mounted) return;
    setState(() {
      _busy = false;
      _progress = null;
    });
    await _showImportSummary(rows);
  }

  Future<void> _showImportSummary(List<ImportResultRow> rows) async {
    final success = rows.where((r) => r.kind == ImportRowKind.success).length;
    final duplicate = rows
        .where((r) => r.kind == ImportRowKind.duplicate)
        .length;
    final storedNoText = rows
        .where((r) => r.kind == ImportRowKind.storedNoText)
        .length;
    final failed = rows.where((r) => r.kind == ImportRowKind.failed).length;

    if (!mounted) return;
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
                _summaryLine(
                  Icons.check_circle,
                  MedMe.teal,
                  '成功识别入库 $success 份',
                ),
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
                _summaryLine(
                  Icons.error_outline,
                  MedMe.danger,
                  '未能处理 $failed 份',
                ),
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

  // ---------------------------------------------------------------------
  // 导出时间线(未加密 HTML)。P6 才做日期区间筛选——FFI 目前无日期参数,
  // 这里如实全量导出,并在说明里提前告知,不做假的日期选择器。
  // ---------------------------------------------------------------------

  Future<void> _exportTimeline() async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('导出时间线'),
        content: const Text(
          '导出完整病历时间线为可打印文件(HTML),未加密,用浏览器打开后可直接打印或另存为 PDF,'
          '适合报销或给医生留档。\n\n当前导出的是全部记录;按时间段筛选导出即将支持。',
          style: TextStyle(fontSize: 13.5, height: 1.5),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('导出并分享'),
          ),
        ],
      ),
    );
    if (confirmed != true) return;

    setState(() {
      _busy = true;
      _progress = '正在生成导出文件…';
    });
    try {
      final result = await exportTimelineHtml();
      if (!mounted) return;
      setState(() {
        _busy = false;
        _progress = null;
      });
      await SharePlus.instance.share(
        ShareParams(files: [XFile(result.path)], subject: 'MedMe 病历时间线导出'),
      );
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _busy = false;
        _progress = null;
      });
      await _showError('导出失败', '$e');
    }
  }

  // ---------------------------------------------------------------------
  // 加密分享给医生:先选有效期,再生成——口令与文件分开告知(口令另发)。
  // ---------------------------------------------------------------------

  Future<void> _startEncryptedShare() async {
    var selectedDays = 7;
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => StatefulBuilder(
        builder: (context, setDialogState) => AlertDialog(
          title: const Text('加密分享给医生'),
          content: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Text(
                '生成一份端到端加密文件和一串口令;对方需要口令才能打开,全程不经过任何服务器。',
                style: TextStyle(
                  fontSize: 13.5,
                  color: MedMe.faint,
                  height: 1.5,
                ),
              ),
              const SizedBox(height: 16),
              const Text(
                '有效期',
                style: TextStyle(fontSize: 13, fontWeight: FontWeight.w600),
              ),
              const SizedBox(height: 8),
              SegmentedButton<int>(
                segments: const [
                  ButtonSegment(value: 7, label: Text('7 天')),
                  ButtonSegment(value: 30, label: Text('30 天')),
                  ButtonSegment(value: 90, label: Text('90 天')),
                ],
                selected: {selectedDays},
                onSelectionChanged: (s) =>
                    setDialogState(() => selectedDays = s.first),
              ),
            ],
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.of(context).pop(false),
              child: const Text('取消'),
            ),
            FilledButton(
              onPressed: () => Navigator.of(context).pop(true),
              child: const Text('生成分享'),
            ),
          ],
        ),
      ),
    );
    if (confirmed != true) return;

    setState(() {
      _busy = true;
      _progress = '正在生成端到端加密分享…';
    });
    try {
      final result = await createShare(expiresDays: selectedDays);
      if (!mounted) return;
      setState(() {
        _busy = false;
        _progress = null;
      });
      await _showShareResult(result);
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _busy = false;
        _progress = null;
      });
      await _showError('生成分享失败', '$e');
    }
  }

  Future<void> _showShareResult(ShareResultDto result) async {
    if (!mounted) return;
    await showDialog<void>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('加密分享已生成'),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              '共 ${result.recordCount} 份记录,已打包为端到端加密文件。',
              style: const TextStyle(fontSize: 13.5, color: MedMe.faint),
            ),
            const SizedBox(height: 10),
            const Text(
              '把文件发给医生,口令请用不同渠道另发(比如短信、电话告知);'
              '对方打开文件、输入口令即可查看,数据始终端到端加密。',
              style: TextStyle(fontSize: 13.5, color: MedMe.faint, height: 1.5),
            ),
            const SizedBox(height: 14),
            Container(
              padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
              decoration: BoxDecoration(
                color: MedMe.bg,
                borderRadius: BorderRadius.circular(10),
                border: Border.all(color: MedMe.line),
              ),
              child: Row(
                children: [
                  const Text(
                    '口令',
                    style: TextStyle(
                      fontSize: 11,
                      fontWeight: FontWeight.w700,
                      color: MedMe.faint,
                    ),
                  ),
                  const SizedBox(width: 10),
                  Expanded(
                    child: Text(
                      result.passphrase,
                      style: const TextStyle(
                        fontSize: 16,
                        fontWeight: FontWeight.w700,
                        letterSpacing: 0.5,
                      ),
                    ),
                  ),
                  IconButton(
                    tooltip: '复制口令',
                    icon: const Icon(Icons.copy, size: 18),
                    onPressed: () async {
                      await Clipboard.setData(
                        ClipboardData(text: result.passphrase),
                      );
                      if (context.mounted) {
                        ScaffoldMessenger.of(
                          context,
                        ).showSnackBar(const SnackBar(content: Text('口令已复制')));
                      }
                    },
                  ),
                ],
              ),
            ),
          ],
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(),
            child: const Text('关闭'),
          ),
          FilledButton.icon(
            icon: const Icon(Icons.ios_share),
            label: const Text('分享文件'),
            onPressed: () async {
              await SharePlus.instance.share(
                ShareParams(files: [XFile(result.path)], subject: 'MedMe 加密病历'),
              );
            },
          ),
        ],
      ),
    );
  }

  Future<void> _showError(String title, String message) => showDialog<void>(
    context: context,
    builder: (context) => AlertDialog(
      title: Text(title),
      content: Text(message),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('知道了'),
        ),
      ],
    ),
  );

  // ---------------------------------------------------------------------
  // 界面
  // ---------------------------------------------------------------------

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('导入 · 导出')),
      body: ListView(
        padding: const EdgeInsets.fromLTRB(16, 16, 16, 32),
        children: [
          if (_progress != null) _progressBanner(_progress!),
          _sectionTitle('添加记录'),
          const SizedBox(height: 8),
          _actionGroup([
            _actionRow(
              icon: Icons.camera_alt_outlined,
              title: '拍照',
              subtitle: '用相机拍摄病历、化验单、报告',
              onTap: _busy ? null : _pickFromCamera,
            ),
            _actionRow(
              icon: Icons.photo_library_outlined,
              title: '从相册选',
              subtitle: '从手机相册选择已有照片,可多选',
              onTap: _busy ? null : _pickFromGallery,
            ),
            _actionRow(
              icon: Icons.folder_open_outlined,
              title: '选择文件',
              subtitle: 'PDF、TXT 或图片文件,可多选',
              onTap: _busy ? null : _pickFiles,
            ),
          ]),
          const SizedBox(height: 24),
          _sectionTitle('导出与分享'),
          const SizedBox(height: 8),
          _actionGroup([
            _actionRow(
              icon: Icons.description_outlined,
              title: '导出时间线(HTML)',
              subtitle: '完整病历时间线,未加密,可打印或自存',
              onTap: _busy ? null : _exportTimeline,
            ),
            _actionRow(
              icon: Icons.lock_outline,
              title: '加密分享给医生',
              subtitle: '生成加密文件与口令,凭口令查看',
              onTap: _busy ? null : _startEncryptedShare,
            ),
          ]),
        ],
      ),
    );
  }

  Widget _progressBanner(String text) => Container(
    margin: const EdgeInsets.only(bottom: 16),
    padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
    decoration: BoxDecoration(
      color: MedMe.tealSoft,
      borderRadius: BorderRadius.circular(12),
    ),
    child: Row(
      children: [
        const SizedBox(
          width: 18,
          height: 18,
          child: CircularProgressIndicator(strokeWidth: 2.4, color: MedMe.teal),
        ),
        const SizedBox(width: 12),
        Expanded(
          child: Text(
            text,
            style: const TextStyle(
              fontSize: 14,
              fontWeight: FontWeight.w600,
              color: MedMe.tealDark,
            ),
          ),
        ),
      ],
    ),
  );

  Widget _sectionTitle(String text) => Padding(
    padding: const EdgeInsets.only(left: 4, bottom: 4),
    child: Text(
      text,
      style: const TextStyle(
        fontSize: 13,
        fontWeight: FontWeight.w700,
        color: MedMe.faint,
        letterSpacing: 0.4,
      ),
    ),
  );

  Widget _actionGroup(List<Widget> rows) => Card(
    child: Column(
      children: [
        for (var i = 0; i < rows.length; i++) ...[
          rows[i],
          if (i != rows.length - 1) const Divider(height: 1, color: MedMe.line),
        ],
      ],
    ),
  );

  Widget _actionRow({
    required IconData icon,
    required String title,
    required String subtitle,
    required VoidCallback? onTap,
  }) {
    return InkWell(
      onTap: onTap,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 14),
        child: Row(
          children: [
            Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                color: MedMe.tealSoft,
                borderRadius: BorderRadius.circular(11),
              ),
              child: Icon(icon, color: MedMe.teal, size: 22),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    title,
                    style: const TextStyle(
                      fontSize: 16,
                      fontWeight: FontWeight.w600,
                      color: MedMe.ink,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    subtitle,
                    style: const TextStyle(fontSize: 13, color: MedMe.faint),
                  ),
                ],
              ),
            ),
            const Icon(Icons.chevron_right, color: MedMe.faint),
          ],
        ),
      ),
    );
  }
}
