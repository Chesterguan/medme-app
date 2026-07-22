import 'dart:async';

import 'package:flutter/material.dart';

import 'package:mobile_flutter/ephemeral_session.dart';
import 'package:mobile_flutter/import_flow.dart'
    show ImportChoice, pickImportItems, ingestPendingItems;
import 'package:mobile_flutter/screens/import_helpers.dart';
import 'package:mobile_flutter/src/rust/api/dto.dart';
import 'package:mobile_flutter/theme.dart';
import 'package:mobile_flutter/widgets/share_result_dialog.dart';

import 'consent_screen.dart';

/// 代建档分享文件的有效期(天)。这台设备本身用完即焚,这个天数只约束「病人手里
/// 那份加密文件的口令还能用多久」——给病人留足取回/打开的时间,与正常导出分享的
/// 「7/30/90 天」选项同量级,取中间档,不额外弹一轮选择打断诊室现场的节奏。
const int kProxyShareExpiresDays = 30;

enum _ProxyPhase { consent, capture, preview, delivering }

/// 「为病人代建档」全屏流程(医生/护士专用,Phase 1:本地交付,不含云)。
/// 同意(签名/按住确认)→ 临时会话(即焚)→ 拍摄纸质材料 → 预览 → 生成加密文件
/// 交付给病人 → 即焚退出。全程只碰独立的 Rust `EPHEMERAL` cell
/// ([EphemeralSession]),绝不写入医生自己的档案——橙色 chrome + 顶部常驻横幅是
/// 每一屏都在的信号,提醒「这不是我的箱」。
class ProxyIntakeFlow extends StatefulWidget {
  const ProxyIntakeFlow({super.key});

  @override
  State<ProxyIntakeFlow> createState() => _ProxyIntakeFlowState();
}

class _ProxyIntakeFlowState extends State<ProxyIntakeFlow> {
  _ProxyPhase _phase = _ProxyPhase.consent;
  bool _sessionStarted = false;
  bool _delivered = false;
  ConsentDto? _consent;
  List<TimelineGroupDto> _preview = const [];
  int _capturedCount = 0;
  bool _busy = false;
  String? _progress;

  @override
  void dispose() {
    // 兜底双保险:任何退出路径(系统返回手势、异常)都确保即焚。幂等——已交付
    // 或从未开始过会话时是 no-op(见 [EphemeralSession.wipe])。
    if (_sessionStarted && !_delivered) {
      unawaited(EphemeralSession.wipe());
    }
    super.dispose();
  }

  /// iOS(尤其 iPad)系统分享面板需要非零锚点矩形,与 `export_screen.dart` 同一理由。
  Rect _shareOrigin() {
    final box = context.findRenderObject() as RenderBox?;
    if (box != null && box.hasSize && !box.size.isEmpty) {
      return box.localToGlobal(Offset.zero) & box.size;
    }
    return const Rect.fromLTWH(0, 0, 1, 1);
  }

  Future<void> _onConsentGiven(ConsentDto consent) async {
    setState(() {
      _busy = true;
      _progress = '正在准备…';
    });
    try {
      await EphemeralSession.begin();
      // 标记必须紧跟在 begin() 成功之后、任何 `mounted` 检查之前:若用户在
      // begin() 这一小段 await 期间就退出了(PopScope/dispose 会在那一刻检查
      // `_sessionStarted`),必须已经能看到「会话已开始」,才会触发即焚——否则
      // 磁盘上的会话目录 + Rust 侧 EPHEMERAL cell 会一直留到下次启动 sweep。
      _sessionStarted = true;
      _consent = consent;
      if (!mounted) {
        // 组件已在这段 await 期间被卸载(用户退出了):这里兜底即焚,不能指望
        // 已经跑过的 dispose() 再补一次(那时 _sessionStarted 还是 false)。
        await EphemeralSession.wipe();
        return;
      }
      setState(() {
        _busy = false;
        _progress = null;
        _phase = _ProxyPhase.capture;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _busy = false;
        _progress = null;
      });
      await _showError('开始会话失败', '$e');
    }
  }

  /// 取消/退出:即焚(若已开始过会话)后关闭整个全屏路由。顶部横幅、同意屏底部
  /// 「不同意」都走这条路径。
  Future<void> _cancelAndExit() async {
    if (_sessionStarted) await EphemeralSession.wipe();
    _delivered = true; // 挡住 dispose 里再 wipe 一次(已经手动 wipe 过)
    if (mounted) Navigator.of(context).pop();
  }

  Future<void> _pickCaptureSource() async {
    final choice = await showModalBottomSheet<ImportChoice>(
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
                  '拍摄病历材料',
                  style: TextStyle(fontSize: 16, fontWeight: FontWeight.w700),
                ),
              ),
            ),
            ListTile(
              leading: const Icon(
                Icons.photo_camera_outlined,
                color: MedMe.proxyOrange,
                size: 28,
              ),
              title: const Text('拍照', style: TextStyle(fontWeight: FontWeight.w600)),
              subtitle: const Text(
                '对着化验单、处方拍一张,自动识别上面的文字',
                style: TextStyle(color: MedMe.faint),
              ),
              onTap: () => Navigator.of(context).pop(ImportChoice.camera),
            ),
            ListTile(
              leading: const Icon(
                Icons.folder_open_outlined,
                color: MedMe.proxyOrange,
                size: 28,
              ),
              title: const Text('选择文件', style: TextStyle(fontWeight: FontWeight.w600)),
              subtitle: const Text(
                '已有的 PDF、图片',
                style: TextStyle(color: MedMe.faint),
              ),
              onTap: () => Navigator.of(context).pop(ImportChoice.files),
            ),
            const SizedBox(height: 8),
          ],
        ),
      ),
    );
    if (choice == null || !mounted) return;

    // 等 bottom sheet 关闭动画播完——与 `import_flow.showImportSheet` 同一时序
    // 理由(文档扫描器靠 `rootViewController.present`,sheet 未完全退场会被挡下、
    // 静默失败)。
    await Future<void>.delayed(const Duration(milliseconds: 350));
    if (!mounted) return;

    final items = await pickImportItems(choice);
    if (items.isEmpty || !mounted) return;
    await _ingest(items);
  }

  Future<void> _ingest(List<PendingImport> items) async {
    setState(() {
      _busy = true;
      _progress = '正在处理 1/${items.length}…';
    });
    final result = await ingestPendingItems(
      items,
      ingestImageWithText: EphemeralSession.ingestImageWithText,
      ingestBytes: EphemeralSession.ingestBytes,
      // Phase 1:临时会话不做扫描版 PDF 的 OCR 回填(仅存原件),设计范围内。
      backfillPdfText: null,
      onProgress: (done, total) {
        if (mounted) setState(() => _progress = '正在处理 ${done + 1}/$total…');
      },
    );
    if (!mounted) return;
    final failed = result.rows.where((r) => r.kind == ImportRowKind.failed).length;
    setState(() {
      _busy = false;
      _progress = null;
      _capturedCount += result.rows.length - failed;
    });
    if (failed > 0) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text('有 $failed 份未能处理,可重拍')));
    }
  }

  Future<void> _goToPreview() async {
    setState(() {
      _busy = true;
      _progress = '正在整理…';
    });
    try {
      final groups = await EphemeralSession.loadPreview();
      if (!mounted) return;
      setState(() {
        _preview = groups;
        _busy = false;
        _progress = null;
        _phase = _ProxyPhase.preview;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _busy = false;
        _progress = null;
      });
      await _showError('加载预览失败', '$e');
    }
  }

  Future<void> _deliver() async {
    final consent = _consent;
    if (consent == null) return;
    setState(() {
      _busy = true;
      _progress = '正在生成端到端加密文件…';
      _phase = _ProxyPhase.delivering;
    });
    try {
      final result = await EphemeralSession.createShare(
        expiresDays: kProxyShareExpiresDays,
        consent: consent,
      );
      if (!mounted) return;
      setState(() {
        _busy = false;
        _progress = null;
      });
      await showShareResultDialog(
        context,
        result,
        shareOrigin: _shareOrigin,
        shareSubject: 'MedMe 病历(代建档)',
        deliveryHint: '请把这份文件交给病人本人保管,口令请当面口头告知或用不同渠道另发;'
            '对方打开文件、输入口令即可查看,数据始终端到端加密。这台设备不会留底。',
      );
      if (!mounted) return;
      _delivered = true;
      await EphemeralSession.wipe();
      if (mounted) Navigator.of(context).pop();
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _busy = false;
        _progress = null;
        _phase = _ProxyPhase.preview;
      });
      await _showError('生成分享失败', '$e');
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
      onPopInvokedWithResult: (didPop, result) async {
        if (didPop) return;
        await _cancelAndExit();
      },
      child: Scaffold(
        backgroundColor: MedMe.bg,
        body: SafeArea(
          child: Column(
            children: [
              _ProxyBanner(onClose: _busy ? null : _cancelAndExit),
              Expanded(child: _buildBody(context)),
            ],
          ),
        ),
      ),
    );
  }

  Widget _buildBody(BuildContext context) {
    switch (_phase) {
      case _ProxyPhase.consent:
        return ConsentScreen(onAgreed: _onConsentGiven, onCancel: _cancelAndExit);
      case _ProxyPhase.capture:
        return _CaptureStep(
          busy: _busy,
          progress: _progress,
          capturedCount: _capturedCount,
          onCapture: _pickCaptureSource,
          onDone: _capturedCount > 0 ? _goToPreview : null,
        );
      case _ProxyPhase.preview:
        return _PreviewStep(
          groups: _preview,
          busy: _busy,
          progress: _progress,
          onCaptureMore: _pickCaptureSource,
          onDeliver: _deliver,
        );
      case _ProxyPhase.delivering:
        return const Center(
          child: CircularProgressIndicator(color: MedMe.proxyOrange),
        );
    }
  }
}

/// 顶部常驻横幅:每一屏都在,一眼分清「这不是我的箱」。
class _ProxyBanner extends StatelessWidget {
  const _ProxyBanner({required this.onClose});

  final VoidCallback? onClose;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: double.infinity,
      color: MedMe.proxyOrange,
      padding: const EdgeInsets.fromLTRB(16, 10, 8, 10),
      child: Row(
        children: [
          const Icon(Icons.info_outline, color: Colors.white, size: 18),
          const SizedBox(width: 8),
          const Expanded(
            child: Text(
              '为他人代建档 · 用完即焚 · 不会存入你的档案',
              style: TextStyle(
                color: Colors.white,
                fontSize: 13,
                fontWeight: FontWeight.w700,
              ),
            ),
          ),
          IconButton(
            onPressed: onClose,
            icon: const Icon(Icons.close, color: Colors.white, size: 20),
            tooltip: '取消并退出',
          ),
        ],
      ),
    );
  }
}

class _CaptureStep extends StatelessWidget {
  const _CaptureStep({
    required this.busy,
    required this.progress,
    required this.capturedCount,
    required this.onCapture,
    required this.onDone,
  });

  final bool busy;
  final String? progress;
  final int capturedCount;
  final VoidCallback onCapture;
  final VoidCallback? onDone;

  @override
  Widget build(BuildContext context) {
    return Stack(
      children: [
        Center(
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 28),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                const Icon(
                  Icons.document_scanner_outlined,
                  color: MedMe.proxyOrange,
                  size: 56,
                ),
                const SizedBox(height: 16),
                Text(
                  capturedCount == 0 ? '拍下病人的纸质病历' : '已拍 $capturedCount 份',
                  style: const TextStyle(fontSize: 18, fontWeight: FontWeight.w700),
                ),
                const SizedBox(height: 8),
                const Text(
                  '化验单、处方、检查报告都可以,可以分多次拍摄',
                  textAlign: TextAlign.center,
                  style: TextStyle(color: MedMe.faint),
                ),
                const SizedBox(height: 28),
                SizedBox(
                  width: double.infinity,
                  height: 50,
                  child: FilledButton.icon(
                    style: FilledButton.styleFrom(backgroundColor: MedMe.proxyOrange),
                    onPressed: busy ? null : onCapture,
                    icon: const Icon(Icons.camera_alt_outlined),
                    label: Text(capturedCount == 0 ? '开始拍摄' : '继续拍摄'),
                  ),
                ),
                if (onDone != null) ...[
                  const SizedBox(height: 12),
                  SizedBox(
                    width: double.infinity,
                    height: 50,
                    child: OutlinedButton(
                      onPressed: busy ? null : onDone,
                      child: const Text('拍完了,去预览'),
                    ),
                  ),
                ],
              ],
            ),
          ),
        ),
        if (busy)
          Positioned.fill(
            child: ColoredBox(
              color: Colors.black26,
              child: Center(
                child: Card(
                  child: Padding(
                    padding: const EdgeInsets.all(20),
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        const SizedBox(
                          width: 22,
                          height: 22,
                          child: CircularProgressIndicator(strokeWidth: 2.5),
                        ),
                        const SizedBox(width: 16),
                        Text(progress ?? '处理中…'),
                      ],
                    ),
                  ),
                ),
              ),
            ),
          ),
      ],
    );
  }
}

class _PreviewStep extends StatelessWidget {
  const _PreviewStep({
    required this.groups,
    required this.busy,
    required this.progress,
    required this.onCaptureMore,
    required this.onDeliver,
  });

  final List<TimelineGroupDto> groups;
  final bool busy;
  final String? progress;
  final VoidCallback onCaptureMore;
  final VoidCallback onDeliver;

  /// 铺平就诊组/独立文档为一份纯清单——预览屏只需要「拍了什么」,不需要档案屏
  /// 那套就诊分组展示。与 `archive_screen.dart` 的 `_allDocs` 同一模式匹配写法。
  static List<DocumentSummaryDto> _flatten(List<TimelineGroupDto> groups) {
    final out = <DocumentSummaryDto>[];
    for (final g in groups) {
      switch (g) {
        case TimelineGroupDto_Encounter(:final docs):
          out.addAll(docs);
        case TimelineGroupDto_Document(:final doc):
          out.add(doc);
      }
    }
    return out;
  }

  @override
  Widget build(BuildContext context) {
    final docs = _flatten(groups);
    return Stack(
      children: [
        Column(
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 16, 16, 8),
              child: Align(
                alignment: Alignment.centerLeft,
                child: Text(
                  '核对一下,共 ${docs.length} 份',
                  style: const TextStyle(fontSize: 16, fontWeight: FontWeight.w700),
                ),
              ),
            ),
            Expanded(
              child: docs.isEmpty
                  ? const Center(child: Text('还没有拍摄任何内容', style: TextStyle(color: MedMe.faint)))
                  : ListView.separated(
                      padding: const EdgeInsets.symmetric(horizontal: 16),
                      itemCount: docs.length,
                      separatorBuilder: (_, _) => const SizedBox(height: 8),
                      itemBuilder: (context, i) {
                        final d = docs[i];
                        final label = kDocTypeLabel[d.docType] ?? d.docType;
                        return Card(
                          child: ListTile(
                            title: Text(d.title ?? label),
                            subtitle: Text(label),
                          ),
                        );
                      },
                    ),
            ),
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 8, 16, 16),
              child: Row(
                children: [
                  Expanded(
                    child: OutlinedButton(
                      onPressed: busy ? null : onCaptureMore,
                      child: const Text('再拍一份'),
                    ),
                  ),
                  const SizedBox(width: 12),
                  Expanded(
                    flex: 2,
                    child: FilledButton(
                      style: FilledButton.styleFrom(backgroundColor: MedMe.proxyOrange),
                      onPressed: busy || docs.isEmpty ? null : onDeliver,
                      child: const Text('生成加密文件,交给病人'),
                    ),
                  ),
                ],
              ),
            ),
          ],
        ),
        if (busy)
          Positioned.fill(
            child: ColoredBox(
              color: Colors.black26,
              child: Center(
                child: Card(
                  child: Padding(
                    padding: const EdgeInsets.all(20),
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        const SizedBox(
                          width: 22,
                          height: 22,
                          child: CircularProgressIndicator(strokeWidth: 2.5),
                        ),
                        const SizedBox(width: 16),
                        Text(progress ?? '处理中…'),
                      ],
                    ),
                  ),
                ),
              ),
            ),
          ),
      ],
    );
  }
}
