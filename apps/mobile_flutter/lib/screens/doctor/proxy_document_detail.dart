import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:pdfx/pdfx.dart';

import 'package:mobile_flutter/ephemeral_session.dart';
import 'package:mobile_flutter/screens/import_helpers.dart' show kDocTypeLabel;
import 'package:mobile_flutter/src/rust/api/dto.dart';
import 'package:mobile_flutter/theme.dart';
import 'package:mobile_flutter/widgets/report_content.dart';

/// [ProxyDocumentDetailScreen] 弹出时告诉调用方(待确认列表屏)接下来该做什么:
/// [none] 什么都没变(用户直接返回);[changed] 确认或删除了这一份,列表需要重新拉
/// `loadPreview`/`summary`/`confirmedMap` 刷新;[retake] 这一份已被删除且调用方
/// 应紧接着重新弹「拍照/相册/文件」采集入口——由列表屏统一编排(复用它已有的采集
/// 方法),本屏自己不碰采集逻辑,避免两处维护同一套 `pickImportItems` 调用。
enum ProxyDetailResult { none, changed, retake }

/// 待确认列表「点进一份」的详情屏(医生代拍流程专用)——**与 `document_detail.dart`
/// 是独立副本,不是共享组件**:数据来自临时会话箱([EphemeralSession])而不是医生
/// 自己的档案([EphemeralSession] 全程只碰独立的 Rust `vault_ephemeral` cell,与
/// `document_detail.dart` 读的 `api::vault` 完全平行、互不可见)。宁可这份代码与
/// `document_detail.dart` 重复大半,也不去改那个文件抽公共组件——保持「不碰普通人
/// 模式一行代码」这条硬规矩在这两个文件上都显而易见成立。
///
/// 布局复用 `document_detail.dart` 的呈现方式:原件图/PDF + 识别文本(`ReportContent`)。
/// 底部按钮换成本流程要的三个动作:确认这一份 / 删除 / 重拍。
class ProxyDocumentDetailScreen extends StatefulWidget {
  const ProxyDocumentDetailScreen({
    super.key,
    required this.docId,
    required this.initiallyConfirmed,
  });

  final int docId;
  /// 打开详情页那一刻,列表屏已知的确认状态(列表屏已经拉过一次
  /// `EphemeralSession.confirmedMap()`,这里不必再多一次 FFI 往返去问同一件事)。
  final bool initiallyConfirmed;

  @override
  State<ProxyDocumentDetailScreen> createState() =>
      _ProxyDocumentDetailScreenState();
}

class _ProxyDocumentDetailScreenState
    extends State<ProxyDocumentDetailScreen> {
  late final Future<DocumentDetailDto> _future = EphemeralSession.getDocument(
    widget.docId,
  );
  late final bool _confirmed = widget.initiallyConfirmed;
  bool _busy = false;

  /// 删除这份文档(收错/拍花了)。确认后不可撤销。
  Future<void> _delete() async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('删除这份?'),
        content: const Text('会从这次代拍里移除,原始照片/文件一并删除,不可撤销。'),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('取消'),
          ),
          FilledButton(
            style: FilledButton.styleFrom(backgroundColor: MedMe.danger),
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('删除'),
          ),
        ],
      ),
    );
    if (ok != true || !mounted) return;
    setState(() => _busy = true);
    try {
      await EphemeralSession.deleteDocument(widget.docId);
      if (mounted) Navigator.of(context).pop(ProxyDetailResult.changed);
    } catch (e) {
      if (!mounted) return;
      setState(() => _busy = false);
      await _showError('删除失败', '$e');
    }
  }

  /// 重拍:这一份拍得不好(糊/切歪/拍错页),删掉后回列表屏由它重新弹采集入口。
  Future<void> _retake() async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('重新拍摄这一份?'),
        content: const Text('原有的这份会被删除,接下来会重新弹出拍照/选择入口。'),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('重拍'),
          ),
        ],
      ),
    );
    if (ok != true || !mounted) return;
    setState(() => _busy = true);
    try {
      await EphemeralSession.deleteDocument(widget.docId);
      if (mounted) Navigator.of(context).pop(ProxyDetailResult.retake);
    } catch (e) {
      if (!mounted) return;
      setState(() => _busy = false);
      await _showError('删除失败', '$e');
    }
  }

  /// 核对无误,确认这一份(整份确认,不细到每一项)。
  Future<void> _confirm() async {
    setState(() => _busy = true);
    try {
      await EphemeralSession.setConfirmed(
        documentId: widget.docId,
        confirmed: true,
      );
      if (mounted) Navigator.of(context).pop(ProxyDetailResult.changed);
    } catch (e) {
      if (!mounted) return;
      setState(() => _busy = false);
      await _showError('确认失败', '$e');
    }
  }

  Future<void> _showError(String title, String message) => showDialog<void>(
    context: context,
    builder: (context) => AlertDialog(
      title: Text(title),
      content: Text(message),
      actions: [
        FilledButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('知道了'),
        ),
      ],
    ),
  );

  @override
  Widget build(BuildContext context) {
    return PopScope(
      canPop: false,
      onPopInvokedWithResult: (didPop, result) {
        if (!didPop) Navigator.of(context).pop(ProxyDetailResult.none);
      },
      child: Scaffold(
        appBar: AppBar(
          title: const Text('核对这一份'),
          actions: [
            IconButton(
              icon: const Icon(Icons.camera_alt_outlined),
              tooltip: '重拍',
              onPressed: _busy ? null : _retake,
            ),
            IconButton(
              icon: const Icon(Icons.delete_outline),
              tooltip: '删除',
              onPressed: _busy ? null : _delete,
            ),
          ],
        ),
        bottomNavigationBar: SafeArea(
          child: Padding(
            padding: const EdgeInsets.fromLTRB(16, 8, 16, 12),
            child: _confirmed
                ? Container(
                    padding: const EdgeInsets.symmetric(vertical: 13),
                    decoration: BoxDecoration(
                      color: const Color(0xFFECFDF5),
                      borderRadius: BorderRadius.circular(10),
                    ),
                    child: const Row(
                      mainAxisAlignment: MainAxisAlignment.center,
                      children: [
                        Icon(
                          Icons.check_circle,
                          color: Color(0xFF047857),
                          size: 18,
                        ),
                        SizedBox(width: 8),
                        Text(
                          '已确认',
                          style: TextStyle(
                            color: Color(0xFF047857),
                            fontWeight: FontWeight.w700,
                          ),
                        ),
                      ],
                    ),
                  )
                : FilledButton.icon(
                    onPressed: _busy ? null : _confirm,
                    icon: const Icon(Icons.check),
                    label: const Text('确认这一份'),
                    style: FilledButton.styleFrom(
                      backgroundColor: MedMe.proxyOrange,
                      minimumSize: const Size.fromHeight(48),
                    ),
                  ),
          ),
        ),
        body: FutureBuilder<DocumentDetailDto>(
          future: _future,
          builder: (context, snap) {
            if (snap.connectionState != ConnectionState.done) {
              return const Center(
                child: CircularProgressIndicator(color: MedMe.proxyOrange),
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
            return _ProxyDetailBody(detail: snap.data!);
          },
        ),
      ),
    );
  }
}

class _ProxyDetailBody extends StatelessWidget {
  final DocumentDetailDto detail;
  const _ProxyDetailBody({required this.detail});

  @override
  Widget build(BuildContext context) {
    final doc = detail.document;
    final sf = detail.sourceFile;
    final typeLabel = kDocTypeLabel[doc.docType] ?? doc.docType;

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
                color: MedMe.proxyOrangeSoft,
                borderRadius: BorderRadius.circular(12),
              ),
              child: const Icon(
                Icons.description_outlined,
                color: MedMe.proxyOrange,
              ),
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

        const SizedBox(height: 14),
        OutlinedButton.icon(
          onPressed: () => _openOriginal(context, sf),
          icon: const Icon(Icons.visibility_outlined, size: 18),
          label: const Text('查看原件'),
          style: OutlinedButton.styleFrom(
            foregroundColor: MedMe.proxyOrange,
            side: const BorderSide(color: MedMe.proxyOrange),
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
          builder: (_) => _ProxyImageViewerScreen(sourceFileId: sf.id),
        ),
      );
      return;
    }
    if (mime == 'application/pdf') {
      Navigator.of(context).push(
        MaterialPageRoute(
          builder: (_) => _ProxyPdfViewerScreen(sourceFileId: sf.id),
        ),
      );
      return;
    }
    if (mime == 'application/dicom') {
      Navigator.of(context).push(
        MaterialPageRoute(
          builder: (_) => _ProxyDicomViewerScreen(sourceFileId: sf.id),
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
        content: Text('此格式($mime)暂不能在手机上预览,原件已安全保存。'),
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

/// "YYYY-MM-DD",与 `document_detail.dart`/`proxy_intake_flow.dart` 的同名私有
/// helper 同一格式(各文件私有,不跨文件共享——都很小,重复比新增公共 util 更简单)。
String _fmtDate(String? iso) {
  if (iso == null || iso.isEmpty) return '';
  final d = DateTime.tryParse(iso);
  if (d == null) return '';
  return '${d.year}-${d.month.toString().padLeft(2, '0')}-${d.day.toString().padLeft(2, '0')}';
}

/// 图片原件全屏查看(可缩放),字节来自 `EphemeralSession.readSourceBytes`(临时
/// 会话箱,不经 iCloud 物化)。
class _ProxyImageViewerScreen extends StatelessWidget {
  final int sourceFileId;
  const _ProxyImageViewerScreen({required this.sourceFileId});

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
        future: EphemeralSession.readSourceBytes(sourceFileId),
        builder: (context, snap) {
          if (snap.connectionState != ConnectionState.done) {
            return const Center(
              child: CircularProgressIndicator(color: MedMe.proxyOrange),
            );
          }
          if (snap.hasError || !snap.hasData) {
            return const _ProxyViewerFallback(message: '原件加载失败,可稍后重试。');
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

/// PDF 原件全屏查看(可翻页),字节来自 `EphemeralSession.readSourceBytes`。
class _ProxyPdfViewerScreen extends StatefulWidget {
  final int sourceFileId;
  const _ProxyPdfViewerScreen({required this.sourceFileId});

  @override
  State<_ProxyPdfViewerScreen> createState() => _ProxyPdfViewerScreenState();
}

class _ProxyPdfViewerScreenState extends State<_ProxyPdfViewerScreen> {
  PdfController? _controller;
  Object? _error;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    try {
      final bytes = await EphemeralSession.readSourceBytes(
        widget.sourceFileId,
      );
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
          ? const _ProxyViewerFallback(message: '此文件暂不能预览。')
          : _controller == null
          ? const Center(
              child: CircularProgressIndicator(color: MedMe.proxyOrange),
            )
          : PdfView(controller: _controller!, onDocumentError: (_) {}),
    );
  }
}

/// DICOM 原件:渲染锚点切片为 PNG;不支持的压缩格式优雅降级,不崩溃。
class _ProxyDicomViewerScreen extends StatelessWidget {
  final int sourceFileId;
  const _ProxyDicomViewerScreen({required this.sourceFileId});

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
        future: EphemeralSession.renderDicomPng(sourceFileId),
        builder: (context, snap) {
          if (snap.connectionState != ConnectionState.done) {
            return const Center(
              child: CircularProgressIndicator(color: MedMe.proxyOrange),
            );
          }
          if (snap.hasError || !snap.hasData) {
            return const _ProxyViewerFallback(
              message: '此 DICOM 格式暂不能预览(可能是不支持的压缩方式)。',
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
class _ProxyViewerFallback extends StatelessWidget {
  final String message;
  final bool light;
  const _ProxyViewerFallback({required this.message, this.light = true});

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
