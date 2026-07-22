import 'package:flutter/material.dart';
import 'package:share_plus/share_plus.dart';

import 'package:mobile_flutter/src/rust/api/vault.dart';
import 'package:mobile_flutter/theme.dart';
import 'package:mobile_flutter/widgets/share_result_dialog.dart';

import 'qr_share_screen.dart';
import 'proxy/proxy_intake_flow.dart';

/// 底部导航一级 tab「导出·分享」—— 把病历导出成可打印文件(可按日期区间筛选),或
/// 端到端加密分享给医生。手机端只做「轻」的导出/筛选;全文搜索、趋势等「重」功能在
/// 桌面端与医生查看器。导出/加密全在 Rust core(`medme_share`),这里只调 FFI + 分享。
class ExportScreen extends StatefulWidget {
  const ExportScreen({super.key});

  @override
  State<ExportScreen> createState() => _ExportScreenState();
}

class _ExportScreenState extends State<ExportScreen> {
  bool _busy = false;
  String? _progress;

  /// iOS 的分享面板(尤其 iPad)必须知道从哪个位置弹出(popover 锚点矩形),否则
  /// `SharePlus` 抛 `argument must be set {{0,0},{0,0}} must be non-zero`。用本屏渲染框作锚点。
  Rect _shareOrigin() {
    final box = context.findRenderObject() as RenderBox?;
    if (box != null && box.hasSize && !box.size.isEmpty) {
      return box.localToGlobal(Offset.zero) & box.size;
    }
    return const Rect.fromLTWH(0, 0, 1, 1);
  }

  static String _ymd(DateTime d) =>
      '${d.year}-${d.month.toString().padLeft(2, '0')}-${d.day.toString().padLeft(2, '0')}';

  Future<void> _exportTimeline() async {
    DateTime? from;
    DateTime? to;
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => StatefulBuilder(
        builder: (context, setDialog) {
          Future<void> pickDate(bool isFrom) async {
            final now = DateTime.now();
            final picked = await showDatePicker(
              context: context,
              initialDate: (isFrom ? from : to) ?? now,
              firstDate: DateTime(1970),
              lastDate: DateTime(now.year + 1),
            );
            if (picked != null) {
              setDialog(() => isFrom ? from = picked : to = picked);
            }
          }

          return AlertDialog(
            title: const Text('导出时间线'),
            content: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const Text(
                  '导出病历时间线为可打印文件(HTML),未加密,用浏览器打开后可直接打印或另存为 PDF,适合报销或给医生留档。',
                  style: TextStyle(
                    fontSize: 13.5,
                    height: 1.5,
                    color: MedMe.faint,
                  ),
                ),
                const SizedBox(height: 16),
                const Text(
                  '时间范围(可选,留空即导出全部)',
                  style: TextStyle(fontSize: 13, fontWeight: FontWeight.w600),
                ),
                const SizedBox(height: 8),
                Row(
                  children: [
                    Expanded(
                      child: OutlinedButton(
                        onPressed: () => pickDate(true),
                        child: Text('从:${from == null ? '不限' : _ymd(from!)}'),
                      ),
                    ),
                    const SizedBox(width: 8),
                    Expanded(
                      child: OutlinedButton(
                        onPressed: () => pickDate(false),
                        child: Text('到:${to == null ? '不限' : _ymd(to!)}'),
                      ),
                    ),
                  ],
                ),
                if (from != null || to != null)
                  Align(
                    alignment: Alignment.centerRight,
                    child: TextButton(
                      onPressed: () => setDialog(() {
                        from = null;
                        to = null;
                      }),
                      child: const Text('清除范围'),
                    ),
                  ),
              ],
            ),
            actions: [
              TextButton(
                onPressed: () => Navigator.of(context).pop(false),
                child: const Text('取消'),
              ),
              FilledButton(
                onPressed: () {
                  if (from != null && to != null && from!.isAfter(to!)) {
                    ScaffoldMessenger.of(context).showSnackBar(
                      const SnackBar(content: Text('起始日期不能晚于结束日期')),
                    );
                    return;
                  }
                  Navigator.of(context).pop(true);
                },
                child: const Text('导出并分享'),
              ),
            ],
          );
        },
      ),
    );
    if (confirmed != true) return;

    setState(() {
      _busy = true;
      _progress = '正在生成导出文件…';
    });
    try {
      final result = await exportTimelineHtml(
        fromDate: from == null ? null : _ymd(from!),
        toDate: to == null ? null : _ymd(to!),
      );
      if (!mounted) return;
      setState(() {
        _busy = false;
        _progress = null;
      });
      await SharePlus.instance.share(
        ShareParams(
          files: [XFile(result.path)],
          subject: 'MedMe 病历时间线导出',
          sharePositionOrigin: _shareOrigin(),
        ),
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
      await showShareResultDialog(context, result, shareOrigin: _shareOrigin);
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _busy = false;
        _progress = null;
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
    return Scaffold(
      appBar: AppBar(title: const Text('导出 · 分享')),
      body: Stack(
        children: [
          ListView(
            padding: const EdgeInsets.fromLTRB(16, 16, 16, 32),
            children: [
              // 门诊现场最高频的一步放最前:医生扫码,三十秒看懂当下病情。
              _ActionCard(
                icon: Icons.qr_code_2,
                title: '当面给医生看',
                subtitle: '生成二维码,医生用自己手机扫一下就能看到在治疾病、'
                    '关键指标趋势与在用药物。不含原件,需要时你再当场翻给他。',
                buttonLabel: '出示二维码',
                onPressed: _busy
                    ? null
                    : () => Navigator.of(context).push(
                        MaterialPageRoute<void>(
                          builder: (_) => const QrShareScreen(),
                        ),
                      ),
              ),
              const SizedBox(height: 14),
              _ActionCard(
                icon: Icons.description_outlined,
                title: '导出时间线',
                subtitle: '导出可打印文件(HTML),可按日期区间筛选;适合报销、留档或给医生。',
                buttonLabel: '选择范围并导出',
                onPressed: _busy ? null : _exportTimeline,
              ),
              const SizedBox(height: 14),
              _ActionCard(
                icon: Icons.lock_outline,
                title: '加密分享给医生',
                subtitle: '生成端到端加密文件 + 口令,对方需口令才能打开,全程不经服务器。',
                buttonLabel: '生成加密分享',
                onPressed: _busy ? null : _startEncryptedShare,
              ),
              const SizedBox(height: 22),
              const Divider(height: 1, color: MedMe.line),
              const SizedBox(height: 22),
              // 医生/护士专用:代病人拍下纸质材料、当场生成加密文件交给病人本人,
              // 用完即焚——独立橙色 chrome 的全屏流程,一眼分清「这不是我的箱」。
              _ActionCard(
                icon: Icons.medical_information_outlined,
                accentColor: MedMe.proxyOrange,
                title: '为病人代建档',
                subtitle: '医生/护士专用:当面拍下病人的纸质病历,当场生成加密文件交给病人本人,'
                    '不会存进你自己的档案,用完即焚。',
                buttonLabel: '开始为病人代建档',
                onPressed: _busy
                    ? null
                    : () => Navigator.of(context).push(
                        MaterialPageRoute<void>(
                          fullscreenDialog: true,
                          builder: (_) => const ProxyIntakeFlow(),
                        ),
                      ),
              ),
            ],
          ),
          if (_busy)
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
                          Text(_progress ?? '处理中…'),
                        ],
                      ),
                    ),
                  ),
                ),
              ),
            ),
        ],
      ),
    );
  }
}

/// 导出/分享的大动作卡:图标 + 标题 + 说明 + 主按钮。[accentColor] 默认品牌 teal;
/// 「为病人代建档」传警示橙,与普通导出/分享动作在视觉上就有区分。
class _ActionCard extends StatelessWidget {
  const _ActionCard({
    required this.icon,
    required this.title,
    required this.subtitle,
    required this.buttonLabel,
    required this.onPressed,
    this.accentColor = MedMe.teal,
  });

  final IconData icon;
  final String title;
  final String subtitle;
  final String buttonLabel;
  final VoidCallback? onPressed;
  final Color accentColor;

  @override
  Widget build(BuildContext context) {
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(18),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(icon, color: accentColor, size: 26),
                const SizedBox(width: 10),
                Text(
                  title,
                  style: const TextStyle(
                    fontSize: 16,
                    fontWeight: FontWeight.w700,
                    color: MedMe.ink,
                  ),
                ),
              ],
            ),
            const SizedBox(height: 10),
            Text(
              subtitle,
              style: const TextStyle(
                fontSize: 13.5,
                color: MedMe.faint,
                height: 1.5,
              ),
            ),
            const SizedBox(height: 14),
            SizedBox(
              width: double.infinity,
              child: FilledButton(
                style: FilledButton.styleFrom(backgroundColor: accentColor),
                onPressed: onPressed,
                child: Text(buttonLabel),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
