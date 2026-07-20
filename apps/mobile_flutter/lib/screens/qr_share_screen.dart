// 面对面二维码分享:门诊里把手机递给医生扫,三十秒看懂当下病情。
//
// 与「加密分享文件」的分工:那个是整份病历(含原件、影像,医生带走);这个是
// **当下病情** —— 在治的病、关键指标最近几个点、在用的药。要看原件或阅片,
// 患者手机当场翻,不必把整份病历交出去。
//
// 载荷有界(Rust 侧 QrLimits),体积与病历总量无关,永远塞得进一张码。密钥在
// URL 的 `#` 之后,按 HTTP 规范不会发给服务器 —— 医生扫码后只从静态页下载一个
// 空壳查看器,病历数据全程只在两台手机之间。
import 'package:flutter/material.dart';
import 'package:qr_flutter/qr_flutter.dart';

import '../src/rust/api/dto.dart';
import '../src/rust/api/vault.dart';
import '../theme.dart';

/// 医生扫码后打开的查看器地址。数据在 `#` 之后,不会随请求上行。
const _viewerBase = 'https://chesterguan.github.io/medme/viewer/';

class QrShareScreen extends StatefulWidget {
  const QrShareScreen({super.key});

  @override
  State<QrShareScreen> createState() => _QrShareScreenState();
}

class _QrShareScreenState extends State<QrShareScreen> {
  QrShareDto? _share;
  String? _error;

  @override
  void initState() {
    super.initState();
    _generate();
  }

  Future<void> _generate() async {
    try {
      final s = await buildQrShareUrl(baseUrl: _viewerBase);
      if (mounted) setState(() => _share = s);
    } catch (e) {
      if (mounted) setState(() => _error = '$e');
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: Colors.white,
      appBar: AppBar(
        title: const Text('给医生看'),
        backgroundColor: Colors.white,
        surfaceTintColor: Colors.transparent,
      ),
      body: SafeArea(child: Center(child: _body())),
    );
  }

  Widget _body() {
    if (_error != null) {
      return Padding(
        padding: const EdgeInsets.all(28),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Icon(Icons.error_outline, size: 40, color: MedMe.danger),
            const SizedBox(height: 12),
            const Text('生成失败', style: TextStyle(fontWeight: FontWeight.w700)),
            const SizedBox(height: 8),
            Text(
              _error!,
              textAlign: TextAlign.center,
              style: const TextStyle(fontSize: 13, color: MedMe.faint),
            ),
            const SizedBox(height: 16),
            FilledButton(onPressed: _generate, child: const Text('重试')),
          ],
        ),
      );
    }
    final s = _share;
    if (s == null) {
      return const Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          CircularProgressIndicator(),
          SizedBox(height: 14),
          Text('正在整理当下病情…', style: TextStyle(color: MedMe.faint, fontSize: 13)),
        ],
      );
    }

    return SingleChildScrollView(
      padding: const EdgeInsets.fromLTRB(24, 8, 24, 28),
      child: Column(
        children: [
          const Text(
            '请医生扫这个码',
            style: TextStyle(fontSize: 20, fontWeight: FontWeight.w800),
          ),
          const SizedBox(height: 6),
          const Text(
            '把屏幕亮度调高,对着医生的手机相机',
            style: TextStyle(fontSize: 13.5, color: MedMe.faint),
          ),
          const SizedBox(height: 20),
          // 白底 + 留白是二维码可扫性的硬要求,别加装饰。
          Container(
            padding: const EdgeInsets.all(16),
            decoration: BoxDecoration(
              color: Colors.white,
              borderRadius: BorderRadius.circular(16),
              border: Border.all(color: MedMe.line),
            ),
            child: QrImageView(
              data: s.url,
              version: QrVersions.auto,
              size: 280,
              backgroundColor: Colors.white,
              // 医生隔着距离扫,纠错等级留高一点更容易扫上。
              // 注意:Rust 侧 `QR_BINARY_CAPACITY` 是按这个等级(M=2331 字节)定的,
              // 改这里必须同步改那个常量,否则守卫会比实际容量宽 27%。
              errorCorrectionLevel: QrErrorCorrectLevel.M,
            ),
          ),
          const SizedBox(height: 18),
          _summaryChip(s),
          const SizedBox(height: 20),
          Container(
            padding: const EdgeInsets.all(14),
            decoration: BoxDecoration(
              color: MedMe.tealSoft,
              borderRadius: BorderRadius.circular(12),
            ),
            child: const Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  '医生看到的是什么',
                  style: TextStyle(fontWeight: FontWeight.w700, fontSize: 13.5),
                ),
                SizedBox(height: 6),
                Text(
                  '在治的疾病、关键化验的近期趋势、正在吃的药。'
                  '不含原件、影像和更早的记录 —— 那些需要时你在自己手机上给他看。',
                  style: TextStyle(fontSize: 12.5, height: 1.6, color: MedMe.ink),
                ),
                SizedBox(height: 10),
                Text(
                  '这张码就是钥匙:被拍下就等于把这份摘要给了对方,'
                  '看完收起手机即可,不必额外「撤回」。',
                  style: TextStyle(fontSize: 12.5, height: 1.6, color: MedMe.faint),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _summaryChip(QrShareDto s) {
    final text = s.problemCount > 0
        ? '本码含 ${s.problemCount} 个在治问题'
        : '暂未识别到在治问题,医生仍可看到用药与近期变化';
    return Row(
      mainAxisAlignment: MainAxisAlignment.center,
      children: [
        const Icon(Icons.lock_outline, size: 15, color: MedMe.faint),
        const SizedBox(width: 6),
        Flexible(
          child: Text(
            text,
            style: const TextStyle(fontSize: 12.5, color: MedMe.faint),
          ),
        ),
      ],
    );
  }
}
