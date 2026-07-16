import 'dart:io';
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:pdfx/pdfx.dart';

import 'package:mobile_flutter/src/rust/api/dto.dart';
import 'package:mobile_flutter/src/rust/api/vault.dart';
import 'package:mobile_flutter/icloud_bridge.dart';
import 'package:mobile_flutter/review_state.dart';
import 'package:mobile_flutter/theme.dart';
import 'package:mobile_flutter/vault_events.dart';
import 'package:mobile_flutter/widgets/report_content.dart';

// doc_type → 中文标签,与 archive_screen.dart 保持同一份映射(桌面/旧移动端
// 同构,来自 core-model types.rs)。
const Map<String, String> _docLabel = {
  'lab_report': '化验',
  'imaging_report': '影像',
  'discharge_summary': '出院小结',
  'prescription': '处方',
  'clinical_note': '病历',
  'pathology': '病理',
  'surgery': '手术',
  'other': '其他',
  'unknown': '待归类',
};

String _fmtDate(String? iso) {
  if (iso == null || iso.isEmpty) return '';
  final d = DateTime.tryParse(iso);
  if (d == null) return '';
  return '${d.year}-${d.month.toString().padLeft(2, '0')}-${d.day.toString().padLeft(2, '0')}';
}

/// iOS-only:读盘前先确保对象已从 iCloud 下载到本地。开启 iCloud 同步后,`objects/`
/// 里的对象可能被 iCloud 逐出(只剩 `.icloud` 占位符),直接读会失败。先经 Rust 拿
/// 对象绝对路径,再让原生触发按需下载并等待,然后再读。安卓/其它平台无 iCloud,
/// 跳过物化直接读(保持快路径)。物化失败也照常尝试读,由调用方做优雅降级。
Future<void> _ensureMaterialized(int sourceFileId) async {
  if (!Platform.isIOS) return;
  try {
    final path = await sourceFileObjectPath(id: sourceFileId);
    await IcloudBridge.ensureDownloaded(path);
  } catch (_) {
    // 拿路径/下载失败不阻断:继续读盘,失败时上层已有「原件加载失败」降级。
  }
}

/// 「查看原件」读原始字节:iOS 上先物化(防 iCloud 逐出),再 `readSourceBytes`。
Future<Uint8List> _readSourceMaterialized(int sourceFileId) async {
  await _ensureMaterialized(sourceFileId);
  return readSourceBytes(id: sourceFileId);
}

/// 「查看原件」渲染 DICOM:iOS 上先物化(防 iCloud 逐出),再 `renderDicomPng`。
Future<Uint8List> _renderDicomMaterialized(int sourceFileId) async {
  await _ensureMaterialized(sourceFileId);
  return renderDicomPng(id: sourceFileId);
}

/// 文档详情屏:类型/日期/来源 + 识别文本(复用 ReportContent 内容感知渲染)+
/// 查看原件(图片/PDF/DICOM 各自渲染,其余格式优雅降级不崩)。
class DocumentDetailScreen extends StatefulWidget {
  final int docId;
  const DocumentDetailScreen({super.key, required this.docId});

  @override
  State<DocumentDetailScreen> createState() => _DocumentDetailScreenState();
}

class _DocumentDetailScreenState extends State<DocumentDetailScreen> {
  late final Future<DocumentDetailDto> _future = getDocument(id: widget.docId);

  /// 删除这份文档:确认 → FFI 删除 → 通知档案刷新 → 退回上一屏。
  Future<void> _delete() async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('删除这份记录?'),
        content: const Text('将从健康档案移除,此操作不可撤销。'),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('取消'),
          ),
          TextButton(
            style: TextButton.styleFrom(foregroundColor: MedMe.danger),
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('删除'),
          ),
        ],
      ),
    );
    if (ok != true) return;
    try {
      await deleteDocument(documentId: widget.docId);
      bumpVaultRevision();
      if (mounted) Navigator.of(context).pop();
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text('删除失败:$e')));
      }
    }
  }

  /// 确认这份待确认文档无误:移出待确认(去掉红框)→ 通知档案刷新 → 退回。
  Future<void> _confirm() async {
    await ReviewState.instance.markReviewed(widget.docId);
    bumpVaultRevision();
    if (mounted) Navigator.of(context).pop();
  }

  @override
  Widget build(BuildContext context) {
    final pending = ReviewState.instance.isPending(widget.docId);
    return Scaffold(
      appBar: AppBar(
        title: const Text('文档详情'),
        actions: [
          IconButton(
            icon: const Icon(Icons.delete_outline),
            tooltip: '删除',
            onPressed: _delete,
          ),
        ],
      ),
      // 待确认文档:底部「确认无误」栏,核对后一键归档(去掉红框、进标准时间线)。
      bottomNavigationBar: pending
          ? SafeArea(
              child: Padding(
                padding: const EdgeInsets.fromLTRB(16, 8, 16, 12),
                child: FilledButton.icon(
                  onPressed: _confirm,
                  icon: const Icon(Icons.check),
                  label: const Text('确认无误,归入档案'),
                  style: FilledButton.styleFrom(
                    minimumSize: const Size.fromHeight(48),
                  ),
                ),
              ),
            )
          : null,
      body: FutureBuilder<DocumentDetailDto>(
        future: _future,
        builder: (context, snap) {
          if (snap.connectionState != ConnectionState.done) {
            return const Center(
              child: CircularProgressIndicator(color: MedMe.teal),
            );
          }
          if (snap.hasError) {
            return Center(
              child: Padding(
                padding: const EdgeInsets.all(32),
                child: Text(
                  '打开失败:\n${snap.error}',
                  textAlign: TextAlign.center,
                  style: const TextStyle(color: MedMe.faint),
                ),
              ),
            );
          }
          return _DetailBody(detail: snap.data!);
        },
      ),
    );
  }
}

class _DetailBody extends StatelessWidget {
  final DocumentDetailDto detail;
  const _DetailBody({required this.detail});

  @override
  Widget build(BuildContext context) {
    final doc = detail.document;
    final sf = detail.sourceFile;
    final typeLabel = _docLabel[doc.docType] ?? doc.docType;

    // OCR 置信度:换算成患者能看懂的三档,而非裸百分比(与旧 App.tsx 一致)。
    final conf = detail.ocrConfidence;
    final confTier = conf == null
        ? null
        : conf >= 0.9
        ? _ConfTier.high
        : conf >= 0.75
        ? _ConfTier.mid
        : _ConfTier.low;

    return ListView(
      padding: const EdgeInsets.fromLTRB(16, 16, 16, 32),
      children: [
        Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Container(
              width: 40,
              height: 40,
              alignment: Alignment.center,
              decoration: BoxDecoration(
                color: MedMe.tealSoft,
                borderRadius: BorderRadius.circular(12),
              ),
              child: const Icon(Icons.description_outlined, color: MedMe.teal),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    doc.title ?? typeLabel,
                    style: const TextStyle(
                      fontSize: 16,
                      fontWeight: FontWeight.w700,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    [
                      typeLabel,
                      if (doc.docDate != null) _fmtDate(doc.docDate),
                    ].join(' · '),
                    style: const TextStyle(fontSize: 12.5, color: MedMe.faint),
                  ),
                  Text(
                    '来源:${sf.originalName}',
                    style: const TextStyle(fontSize: 12, color: MedMe.faint),
                  ),
                ],
              ),
            ),
          ],
        ),

        if (confTier != null) ...[
          const SizedBox(height: 14),
          _ConfBadge(tier: confTier),
        ],

        const SizedBox(height: 14),
        OutlinedButton.icon(
          onPressed: () => _openOriginal(context, sf),
          icon: const Icon(Icons.visibility_outlined, size: 18),
          label: const Text('查看原件'),
          style: OutlinedButton.styleFrom(
            foregroundColor: MedMe.teal,
            side: const BorderSide(color: MedMe.teal),
            minimumSize: const Size.fromHeight(44),
          ),
        ),

        const SizedBox(height: 20),
        Row(
          children: [
            const Icon(Icons.article_outlined, size: 15, color: MedMe.faint),
            const SizedBox(width: 6),
            Text(
              sf.mimeType.startsWith('image/') ? '识别文本' : '文档内容',
              style: const TextStyle(
                fontSize: 12.5,
                fontWeight: FontWeight.w700,
                color: MedMe.faint,
                letterSpacing: 0.4,
              ),
            ),
          ],
        ),
        const SizedBox(height: 10),
        ReportContent(text: detail.ocrText, docType: doc.docType),
      ],
    );
  }

  Future<void> _openOriginal(BuildContext context, SourceFileMetaDto sf) async {
    final mime = sf.mimeType;
    if (mime.startsWith('image/')) {
      Navigator.of(context).push(
        MaterialPageRoute(
          builder: (_) => _ImageViewerScreen(sourceFileId: sf.id),
        ),
      );
      return;
    }
    if (mime == 'application/pdf') {
      Navigator.of(context).push(
        MaterialPageRoute(
          builder: (_) => _PdfViewerScreen(sourceFileId: sf.id),
        ),
      );
      return;
    }
    if (mime == 'application/dicom') {
      Navigator.of(context).push(
        MaterialPageRoute(
          builder: (_) => _DicomViewerScreen(sourceFileId: sf.id),
        ),
      );
      return;
    }
    // 其余格式手机端无法内联预览——如实告知,原件仍安全保存,不静默空白。
    if (!context.mounted) return;
    await showDialog<void>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('暂不能预览'),
        content: Text('此格式($mime)暂不能在手机上预览,原件已安全保存在健康档案里。'),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx),
            child: const Text('知道了'),
          ),
        ],
      ),
    );
  }
}

enum _ConfTier { high, mid, low }

/// 识别质量徽标:高/中/低三档,比裸百分比更易懂(与旧 App.tsx .conf 一致)。
class _ConfBadge extends StatelessWidget {
  final _ConfTier tier;
  const _ConfBadge({required this.tier});

  @override
  Widget build(BuildContext context) {
    final (bg, fg, icon, text) = switch (tier) {
      _ConfTier.high => (
        const Color(0xFFECFDF5),
        const Color(0xFF047857),
        Icons.check_circle_outline,
        '识别质量:高',
      ),
      _ConfTier.mid => (
        const Color(0xFFFFFBEB),
        const Color(0xFFB45309),
        Icons.error_outline,
        '识别质量:中 · 个别字可能有误,可核对原件',
      ),
      _ConfTier.low => (
        const Color(0xFFFDEAEA),
        const Color(0xFFB42318),
        Icons.error_outline,
        '识别质量:低 · 建议重新拍摄',
      ),
    };
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
      decoration: BoxDecoration(
        color: bg,
        borderRadius: BorderRadius.circular(10),
      ),
      child: Row(
        children: [
          Icon(icon, size: 17, color: fg),
          const SizedBox(width: 8),
          Expanded(
            child: Text(text, style: TextStyle(fontSize: 12.5, color: fg)),
          ),
        ],
      ),
    );
  }
}

/// 图片原件全屏查看(可缩放),字节来自 `readSourceBytes`。
class _ImageViewerScreen extends StatelessWidget {
  final int sourceFileId;
  const _ImageViewerScreen({required this.sourceFileId});

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: Colors.black,
      appBar: AppBar(
        backgroundColor: Colors.black,
        foregroundColor: Colors.white,
        title: const Text('原件'),
      ),
      body: FutureBuilder<Uint8List>(
        future: _readSourceMaterialized(sourceFileId),
        builder: (context, snap) {
          if (snap.connectionState != ConnectionState.done) {
            return const Center(
              child: CircularProgressIndicator(color: MedMe.teal),
            );
          }
          if (snap.hasError || !snap.hasData) {
            return const _ViewerFallback(message: '原件加载失败,已安全保存在档案里,可稍后重试。');
          }
          return PhotoView(
            imageProvider: MemoryImage(snap.data!),
            backgroundDecoration: const BoxDecoration(color: Colors.black),
          );
        },
      ),
    );
  }
}

/// PDF 原件全屏查看(可翻页),字节来自 `readSourceBytes` → `PdfDocument.openData`。
class _PdfViewerScreen extends StatefulWidget {
  final int sourceFileId;
  const _PdfViewerScreen({required this.sourceFileId});

  @override
  State<_PdfViewerScreen> createState() => _PdfViewerScreenState();
}

class _PdfViewerScreenState extends State<_PdfViewerScreen> {
  PdfController? _controller;
  Object? _error;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    try {
      final bytes = await _readSourceMaterialized(widget.sourceFileId);
      if (!mounted) return;
      setState(() {
        _controller = PdfController(document: PdfDocument.openData(bytes));
      });
    } catch (e) {
      if (!mounted) return;
      setState(() => _error = e);
    }
  }

  @override
  void dispose() {
    _controller?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('原件')),
      body: _error != null
          ? const _ViewerFallback(message: '此文件暂不能预览,原件已安全保存在档案里。')
          : _controller == null
          ? const Center(child: CircularProgressIndicator(color: MedMe.teal))
          : PdfView(controller: _controller!, onDocumentError: (_) {}),
    );
  }
}

/// DICOM 原件:渲染锚点切片为 PNG;不支持的压缩格式优雅降级,不崩溃。
class _DicomViewerScreen extends StatelessWidget {
  final int sourceFileId;
  const _DicomViewerScreen({required this.sourceFileId});

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: Colors.black,
      appBar: AppBar(
        backgroundColor: Colors.black,
        foregroundColor: Colors.white,
        title: const Text('影像原件'),
      ),
      body: FutureBuilder<Uint8List>(
        future: _renderDicomMaterialized(sourceFileId),
        builder: (context, snap) {
          if (snap.connectionState != ConnectionState.done) {
            return const Center(
              child: CircularProgressIndicator(color: MedMe.teal),
            );
          }
          if (snap.hasError || !snap.hasData) {
            return const _ViewerFallback(
              message: '此 DICOM 格式暂不能预览(可能是不支持的压缩方式),原件已安全保存。',
              light: false,
            );
          }
          return PhotoView(
            imageProvider: MemoryImage(snap.data!),
            backgroundDecoration: const BoxDecoration(color: Colors.black),
          );
        },
      ),
    );
  }
}

/// 查看原件失败/不支持时的统一降级提示——永远给出如实文案,不留空白。
class _ViewerFallback extends StatelessWidget {
  final String message;
  final bool light;
  const _ViewerFallback({required this.message, this.light = true});

  @override
  Widget build(BuildContext context) {
    final color = light ? MedMe.faint : Colors.white70;
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(32),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.image_not_supported_outlined, size: 40, color: color),
            const SizedBox(height: 12),
            Text(
              message,
              textAlign: TextAlign.center,
              style: TextStyle(color: color, fontSize: 13.5, height: 1.6),
            ),
          ],
        ),
      ),
    );
  }
}
