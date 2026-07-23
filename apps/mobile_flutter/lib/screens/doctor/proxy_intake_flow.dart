import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';

import 'package:mobile_flutter/ephemeral_session.dart';
import 'package:mobile_flutter/import_flow.dart' show ImportChoice, pickImportItems;
import 'package:mobile_flutter/ocr_bridge.dart';
import 'package:mobile_flutter/screens/doctor/consent_screen.dart';
import 'package:mobile_flutter/screens/doctor/doctor_delivery_count.dart';
import 'package:mobile_flutter/screens/doctor/doctor_share_result_dialog.dart';
import 'package:mobile_flutter/screens/doctor/proxy_document_detail.dart';
import 'package:mobile_flutter/screens/doctor/proxy_summary_card.dart';
import 'package:mobile_flutter/screens/import_helpers.dart';
import 'package:mobile_flutter/src/rust/api/dto.dart';
import 'package:mobile_flutter/theme.dart';

/// 代建档分享文件的有效期(天)。这台设备本身用完即焚,这个天数只约束「病人手里
/// 那份加密文件的口令还能用多久」——给病人留足取回/打开的时间,与正常导出分享的
/// 「7/30/90 天」选项同量级,取中间档,不额外弹一轮选择打断诊室现场的节奏。
const int kProxyShareExpiresDays = 30;

enum _ProxyPhase { consent, capture, preview, delivering }

/// 「为病人代建档」全屏流程(医生/护士专用,Phase 1:本地交付,不含云)。
/// 同意(签名/按住确认)→ 临时会话(即焚)→ 采集(拍照/相册/文件,可多轮混合来源
/// 累加)→ **待确认列表**(每份一行,点进去核对原件+识别内容、逐份点「确认这一份」;
/// 可随时「继续采集」再累加更多)→ 生成加密文件交付给病人(摘要只统计已确认的
/// 文档,未确认的原件仍全部进分享包并标注待确认)→ 即焚退出。全程只碰独立的 Rust
/// `vault_ephemeral` cell([EphemeralSession]),绝不写入医生自己的档案——橙色
/// chrome + 顶部常驻横幅是每一屏都在的信号,提醒「这不是我的箱」。
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
  // 文档 id → 是否已确认(用户拍板的最终流程:待确认列表用它渲染「待确认/已确认」
  // 标签)。查不到的 id 一律按「待确认」处理,与 Rust 侧 `ephemeral_confirmed_map`
  // 的默认值语义一致。
  Map<int, bool> _confirmedMap = const {};
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
                Icons.photo_library_outlined,
                color: MedMe.proxyOrange,
                size: 28,
              ),
              title: const Text('从相册选', style: TextStyle(fontWeight: FontWeight.w600)),
              subtitle: const Text(
                '选一张或多张已经拍好的病历照片',
                style: TextStyle(color: MedMe.faint),
              ),
              onTap: () => Navigator.of(context).pop(ImportChoice.gallery),
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
    // 采集完直接进审阅屏(病情摘要 + 逐份识别内容摊开),不再停在采集屏问「继续 / 去
    // 预览」——「继续拍摄」是审阅屏上的一个按钮。让「拍完 → 看到审阅」一步到位。
    if (mounted && _capturedCount > 0) {
      await _goToPreview();
    }
  }

  /// 加载/刷新待确认列表:就诊时间线(铺平成文档清单)+ 病情摘要卡(只统计已确认
  /// 文档)+ 每份文档的确认状态。采集完成后、以及每次从详情页返回(确认/删除/重拍)
  /// 后都调这个来刷新——单一数据源,不另维护一套局部更新逻辑。`_capturedCount`
  /// 顺带用这次拿到的真实文档数覆盖,不再靠调用方手动加减去维持同步。
  Future<void> _goToPreview() async {
    setState(() {
      _busy = true;
      _progress = '正在整理…';
    });
    try {
      final groups = await EphemeralSession.loadPreview();
      final docs = _PendingListStep.flatten(groups);
      final summary = await EphemeralSession.summary();
      final confirmedList = await EphemeralSession.confirmedMap();
      final confirmedMap = <int, bool>{
        for (final c in confirmedList) c.documentId: c.confirmed,
      };
      if (!mounted) return;
      setState(() {
        _preview = groups;
        _summary = summary;
        _confirmedMap = confirmedMap;
        _capturedCount = docs.length;
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

  /// 点进一份的详情页:核对原件 + 识别内容,「确认这一份」/ 删除 / 重拍都在那一屏
  /// 完成(见 `proxy_document_detail.dart`)。回来后按详情页汇报的结果决定下一步:
  /// 有变化(确认或删除)就刷新列表;是「重拍」则刷新后紧接着重新弹采集入口——
  /// 复用现有的 [_pickCaptureSource]/[_ingest] 链路,不在详情页重复一遍采集逻辑。
  Future<void> _openDocument(DocumentSummaryDto doc) async {
    final result = await Navigator.of(context).push<ProxyDetailResult>(
      MaterialPageRoute(
        builder: (_) => ProxyDocumentDetailScreen(
          docId: doc.id,
          initiallyConfirmed: _confirmedMap[doc.id] ?? false,
        ),
      ),
    );
    if (!mounted || result == null || result == ProxyDetailResult.none) {
      return;
    }
    await _goToPreview();
    if (result == ProxyDetailResult.retake) {
      await _pickCaptureSource();
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
        return _PendingListStep(
          groups: _preview,
          summary: _summary,
          confirmedMap: _confirmedMap,
          busy: _busy,
          progress: _progress,
          onCaptureMore: _pickCaptureSource,
          onDeliver: _deliver,
          onOpenDocument: _openDocument,
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

/// 待确认列表:采集完进这一屏。渲染风格复用 `archive_screen.dart` 的时间线
/// 列表(图标+类型色块、标题、日期、副标题),每份一行,不再像上一版那样把识别
/// 内容摊开在列表里——点进一份才看原件 + 识别内容(见 `proxy_document_detail.dart`),
/// 列表本身只负责「核对拍了什么、哪些还没点开确认」。
class _PendingListStep extends StatelessWidget {
  const _PendingListStep({
    required this.groups,
    required this.summary,
    required this.confirmedMap,
    required this.busy,
    required this.progress,
    required this.onCaptureMore,
    required this.onDeliver,
    required this.onOpenDocument,
  });

  final List<TimelineGroupDto> groups;
  final ProxySummaryDto? summary;
  final Map<int, bool> confirmedMap;
  final bool busy;
  final String? progress;
  final VoidCallback onCaptureMore;
  final VoidCallback onDeliver;
  final ValueChanged<DocumentSummaryDto> onOpenDocument;

  /// 铺平就诊组/独立文档为一份纯清单——待确认列表只需要「拍了什么」,不需要档案屏
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
    final confirmedCount = docs.where((d) => confirmedMap[d.id] ?? false).length;
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
                  '共 ${docs.length} 份 · 已确认 $confirmedCount 份',
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
                        // 病情摘要卡:在治的病/关键化验/在用药,只统计「已确认」的
                        // 文档(见 `EphemeralSession.summary`)。没有任何结构化问题
                        // (尚无文档被确认,或全是原始未分类图片)时组件自身收起为
                        // 零高度,不占地方。
                        if (s != null) ProxySummaryCard(summary: s),
                        const Padding(
                          padding: EdgeInsets.fromLTRB(16, 4, 16, 8),
                          child: Text(
                            '逐份核对',
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
                            child: _PendingRow(
                              doc: d,
                              confirmed: confirmedMap[d.id] ?? false,
                              onTap: busy ? null : () => onOpenDocument(d),
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
                      child: const Text('继续采集'),
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

/// 待确认列表一行:类型图标 + 标题/日期/类型 + 「待确认/已确认」状态标签。样式
/// 参照 `archive_screen.dart` 的时间线行(图标底色块 + 标题/副标题两行),状态标签
/// 配色沿用该文件 `_PendingCard`(待确认=danger 红)与 `document_detail.dart`
/// `_ConfBadge` 高档(已确认=绿,`#047857`/`#ECFDF5`)的既有色值,不另发明一套。
class _PendingRow extends StatelessWidget {
  const _PendingRow({
    required this.doc,
    required this.confirmed,
    required this.onTap,
  });

  final DocumentSummaryDto doc;
  final bool confirmed;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final label = kDocTypeLabel[doc.docType] ?? doc.docType;
    final date = _fmtDate(doc.docDate);
    return Material(
      color: MedMe.panel,
      borderRadius: BorderRadius.circular(14),
      child: InkWell(
        onTap: onTap,
        borderRadius: BorderRadius.circular(14),
        child: Container(
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(14),
            border: Border.all(
              color: confirmed ? MedMe.line : MedMe.danger.withValues(alpha: 0.5),
            ),
          ),
          padding: const EdgeInsets.all(12),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Container(
                width: 36,
                height: 36,
                alignment: Alignment.center,
                decoration: BoxDecoration(
                  color: MedMe.proxyOrangeSoft,
                  borderRadius: BorderRadius.circular(10),
                ),
                child: const Icon(
                  Icons.description_outlined,
                  size: 19,
                  color: MedMe.proxyOrange,
                ),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      doc.title ?? label,
                      style: const TextStyle(
                        fontSize: 14.5,
                        fontWeight: FontWeight.w700,
                      ),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                    ),
                    const SizedBox(height: 3),
                    Text(
                      date.isEmpty ? label : '$label · $date',
                      style: const TextStyle(fontSize: 12.5, color: MedMe.faint),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 8),
              _StatusBadge(confirmed: confirmed),
              const SizedBox(width: 4),
              const Icon(Icons.chevron_right, size: 20, color: MedMe.faint),
            ],
          ),
        ),
      ),
    );
  }
}

class _StatusBadge extends StatelessWidget {
  const _StatusBadge({required this.confirmed});

  final bool confirmed;

  @override
  Widget build(BuildContext context) {
    final (bg, fg, text) = confirmed
        ? (const Color(0xFFECFDF5), const Color(0xFF047857), '已确认')
        : (MedMe.danger.withValues(alpha: 0.1), MedMe.danger, '待确认');
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
      decoration: BoxDecoration(color: bg, borderRadius: BorderRadius.circular(6)),
      child: Text(
        text,
        style: TextStyle(fontSize: 11, fontWeight: FontWeight.w700, color: fg),
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
