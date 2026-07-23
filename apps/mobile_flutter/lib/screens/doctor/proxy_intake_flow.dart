import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';

import 'package:mobile_flutter/ephemeral_session.dart';
import 'package:mobile_flutter/import_flow.dart' show ImportChoice, pickImportItems;
import 'package:mobile_flutter/ocr_bridge.dart';
import 'package:mobile_flutter/screens/doctor/consent_screen.dart';
import 'package:mobile_flutter/screens/doctor/doctor_delivery_count.dart';
import 'package:mobile_flutter/screens/doctor/doctor_share_result_dialog.dart';
import 'package:mobile_flutter/screens/doctor/proxy_summary_card.dart';
import 'package:mobile_flutter/screens/import_helpers.dart';
import 'package:mobile_flutter/src/rust/api/dto.dart';
import 'package:mobile_flutter/theme.dart';
import 'package:mobile_flutter/widgets/report_content.dart';

/// 代建档分享文件的有效期(天)。这台设备本身用完即焚,这个天数只约束「病人手里
/// 那份加密文件的口令还能用多久」——给病人留足取回/打开的时间,与正常导出分享的
/// 「7/30/90 天」选项同量级,取中间档,不额外弹一轮选择打断诊室现场的节奏。
const int kProxyShareExpiresDays = 30;

enum _ProxyPhase { consent, capture, preview, delivering }

/// 「为病人代建档」全屏流程(医生/护士专用,Phase 1:本地交付,不含云)。
/// 同意(签名/按住确认)→ 临时会话(即焚)→ 拍摄纸质材料 → 预览 → 生成加密文件
/// 交付给病人 → 即焚退出。全程只碰独立的 Rust `vault_ephemeral` cell
/// ([EphemeralSession]),绝不写入医生自己的档案——橙色 chrome + 顶部常驻横幅是
/// 每一屏都在的信号,提醒「这不是我的箱」。
///
/// **采集本身走现有 `recognizeImageText`(`ocr_bridge.dart`,iOS/安卓各自原生
/// OCR)——本文件不碰、不重写任何 OCR 逻辑**,只是把识别结果落进临时会话箱而不是
/// 医生自己的保险箱(见 [_ingest] 与 `EphemeralSession.ingestImageWithText`)。
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
  ProxySummaryDto? _summary;
  // 文档 id → 识别文本,供审阅屏「逐份识别内容」默认摊开展示(不用点开才看到)。
  Map<int, String> _docTexts = const {};
  int _capturedCount = 0;
  bool _busy = false;
  String? _progress;

  @override
  void dispose() {
    // 兜底双保险:任何退出路径(系统返回手势、异常)都确保即焚。幂等——已交付
    // 或从未开始过会话时是 no-op(见 `EphemeralSession.wipe`)。
    if (_sessionStarted && !_delivered) {
      unawaited(EphemeralSession.wipe());
    }
    super.dispose();
  }

  /// 系统分享面板(尤其 iPad)需要非零锚点矩形,与 `export_screen.dart` 同一理由。
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
      // begin() 这一小段 await 期间就退出了,dispose() 会在那一刻检查
      // `_sessionStarted`,必须已经能看到「会话已开始」才会触发即焚——否则磁盘上
      // 的会话目录 + Rust 侧 cell 会一直留到下次启动 sweep。
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
              subtitle: const Text('已有的 PDF、图片', style: TextStyle(color: MedMe.faint)),
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

  /// 采集落库——**独立实现,不复用 `import_flow.dart` 的 `_runImport`**(那条链路
  /// 落的是医生自己的保险箱)。OCR 识别本身仍是同一个未改动的 [recognizeImageText]
  /// (`ocr_bridge.dart`,iOS/安卓各自原生引擎);唯一差异是落库目的地换成
  /// [EphemeralSession] 的临时会话箱。Phase 1 范围内不做扫描版 PDF 的 OCR 回填
  /// (仅存原件),与「先把核心闭环做对」的取舍一致,不在诊室现场为个别扫描版
  /// PDF 多等一轮渲染。
  Future<void> _ingest(List<PendingImport> items) async {
    setState(() {
      _busy = true;
      _progress = '正在处理 1/${items.length}…';
    });
    var failed = 0;
    for (var i = 0; i < items.length; i++) {
      final item = items[i];
      if (mounted) {
        setState(() => _progress = '正在处理 ${i + 1}/${items.length}…');
      }
      try {
        if (item.isImage) {
          final ocr = await recognizeImageText(item.path);
          final bytes = await File(item.path).readAsBytes();
          await EphemeralSession.ingestImageWithText(
            name: item.name,
            bytes: bytes,
            ocrText: ocr.text,
            confidence: ocr.confidence,
          );
        } else {
          final bytes = await File(item.path).readAsBytes();
          await EphemeralSession.ingestBytes(filename: item.name, data: bytes);
        }
        _capturedCount++;
      } catch (e) {
        debugPrint('[doctor-proxy] ${item.name} 采集失败: $e');
        failed++;
      }
    }
    if (!mounted) return;
    setState(() {
      _busy = false;
      _progress = null;
    });
    if (failed > 0) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text('有 $failed 份未能处理,可重拍')));
    }
  }

  /// 加载/刷新审阅屏:就诊时间线 + 病情摘要卡 + 每份文档的识别文本(默认摊开展示,
  /// 不用点开)。采集完成后、以及每次删除一份文档后都调这个来刷新三样。
  Future<void> _goToPreview() async {
    setState(() {
      _busy = true;
      _progress = '正在整理…';
    });
    try {
      final groups = await EphemeralSession.loadPreview();
      final docs = _PreviewStep.flatten(groups);
      final summary = await EphemeralSession.summary();
      final texts = <int, String>{};
      for (final d in docs) {
        try {
          texts[d.id] = await EphemeralSession.documentText(d.id);
        } catch (e) {
          // 单份取文本失败不阻塞其它文档展示;该份退化为只显示标题/类型。
          debugPrint('[doctor-proxy] 取文档 ${d.id} 识别文本失败: $e');
        }
      }
      if (!mounted) return;
      setState(() {
        _preview = groups;
        _summary = summary;
        _docTexts = texts;
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

  /// 删掉审阅屏里收错/拍花的一份(确认后不可撤销)。删完重新走 [_goToPreview]
  /// 刷新摘要卡 + 文档列表 —— 与「拍完了,去预览」共用同一条加载路径,不另维护
  /// 一套局部更新逻辑。
  Future<void> _deleteDocument(DocumentSummaryDto doc) async {
    final label = doc.title ?? (kDocTypeLabel[doc.docType] ?? doc.docType);
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('删除这份?'),
        content: Text('「$label」会从这次代拍里移除,原始照片/文件一并删除,不可撤销。'),
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
    if (confirmed != true || !mounted) return;
    setState(() {
      _busy = true;
      _progress = '正在删除…';
    });
    try {
      await EphemeralSession.deleteDocument(doc.id);
      if (_capturedCount > 0) _capturedCount--;
      await _goToPreview();
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _busy = false;
        _progress = null;
      });
      await _showError('删除失败', '$e');
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
      // 纯本地计数 +1(不存任何病人数据),见 `doctor_delivery_count.dart`——文件已
      // 生成即算「交付」,不管用户接下来是否还留在这一屏,计数都该 +1。
      await DoctorDeliveryCount.instance.increment();
      if (!mounted) return;
      await showDoctorShareResultDialog(context, result, shareOrigin: _shareOrigin);
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
          summary: _summary,
          docTexts: _docTexts,
          busy: _busy,
          progress: _progress,
          onCaptureMore: _pickCaptureSource,
          onDeliver: _deliver,
          onDeleteDocument: _deleteDocument,
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
    required this.summary,
    required this.docTexts,
    required this.busy,
    required this.progress,
    required this.onCaptureMore,
    required this.onDeliver,
    required this.onDeleteDocument,
  });

  final List<TimelineGroupDto> groups;
  final ProxySummaryDto? summary;
  final Map<int, String> docTexts;
  final bool busy;
  final String? progress;
  final VoidCallback onCaptureMore;
  final VoidCallback onDeliver;
  final ValueChanged<DocumentSummaryDto> onDeleteDocument;

  /// 铺平就诊组/独立文档为一份纯清单——预览屏只需要「拍了什么」,不需要档案屏
  /// 那套就诊分组展示。与 `archive_screen.dart` 的展开模式同一匹配写法。
  static List<DocumentSummaryDto> flatten(List<TimelineGroupDto> groups) {
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
    final docs = flatten(groups);
    final s = summary;
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
                  ? const Center(
                      child: Text('还没有拍摄任何内容', style: TextStyle(color: MedMe.faint)),
                    )
                  : ListView(
                      padding: const EdgeInsets.only(bottom: 16),
                      children: [
                        // 病情摘要卡(选项 b 的核心):在治的病/关键化验/在用药,
                        // 不必点开就看到。没有任何结构化问题(如全是原始未分类图片)
                        // 时组件自身收起为零高度,不占地方。
                        if (s != null) ProxySummaryCard(summary: s),
                        const Padding(
                          padding: EdgeInsets.fromLTRB(16, 4, 16, 8),
                          child: Text(
                            '逐份识别内容',
                            style: TextStyle(
                              fontSize: 13,
                              fontWeight: FontWeight.w700,
                              color: MedMe.faint,
                            ),
                          ),
                        ),
                        for (final d in docs)
                          Padding(
                            padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
                            child: _DocumentCard(
                              doc: d,
                              text: docTexts[d.id],
                              onDelete: busy ? null : () => onDeleteDocument(d),
                            ),
                          ),
                      ],
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

/// 逐份文档卡:标题/类型/日期 + 删除 + 识别内容(复用 `widgets/report_content.dart`
/// 的 `ReportContent`,化验渲染成表格)。**默认展开**(`initiallyExpanded: true`)——
/// 「即便不点开也要看到」,长文本仍可点标题行折叠收起,不强制占满整屏。
class _DocumentCard extends StatelessWidget {
  const _DocumentCard({required this.doc, required this.text, required this.onDelete});

  final DocumentSummaryDto doc;
  /// 识别文本;仍在加载时为 `null`(显示小转圈,不阻塞其它卡片先渲染出来)。
  final String? text;
  final VoidCallback? onDelete;

  @override
  Widget build(BuildContext context) {
    final label = kDocTypeLabel[doc.docType] ?? doc.docType;
    final date = _fmtDate(doc.docDate);
    return Card(
      clipBehavior: Clip.antiAlias,
      child: Theme(
        // 去掉 ExpansionTile 默认的顶/底分割线,贴合卡片圆角边框。
        data: Theme.of(context).copyWith(dividerColor: Colors.transparent),
        child: ExpansionTile(
          initiallyExpanded: true,
          title: Text(
            doc.title ?? label,
            style: const TextStyle(fontWeight: FontWeight.w700, fontSize: 14.5),
          ),
          subtitle: Text(
            date.isEmpty ? label : '$label · $date',
            style: const TextStyle(fontSize: 12, color: MedMe.faint),
          ),
          trailing: IconButton(
            icon: const Icon(Icons.delete_outline, color: MedMe.danger, size: 20),
            tooltip: '删除这份',
            onPressed: onDelete,
          ),
          childrenPadding: const EdgeInsets.fromLTRB(16, 0, 16, 16),
          expandedCrossAxisAlignment: CrossAxisAlignment.start,
          children: [
            if (text == null)
              const Padding(
                padding: EdgeInsets.only(top: 4),
                child: SizedBox(
                  height: 18,
                  width: 18,
                  child: CircularProgressIndicator(strokeWidth: 2),
                ),
              )
            else
              ReportContent(text: text!, docType: doc.docType),
          ],
        ),
      ),
    );
  }
}

/// "YYYY-MM-DD",与 `document_detail.dart` 的同名私有 helper 同一格式(各文件私有,
/// 不跨文件共享——两处都很小,重复比新增一个公共 util 文件更简单)。
String _fmtDate(String? iso) {
  if (iso == null || iso.isEmpty) return '';
  final d = DateTime.tryParse(iso);
  if (d == null) return '';
  return '${d.year}-${d.month.toString().padLeft(2, '0')}-${d.day.toString().padLeft(2, '0')}';
}
