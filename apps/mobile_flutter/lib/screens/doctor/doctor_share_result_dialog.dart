import 'package:flutter/material.dart';
import 'package:flutter/services.dart' show Clipboard, ClipboardData;
import 'package:share_plus/share_plus.dart';

import 'package:mobile_flutter/src/rust/api/dto.dart';
import 'package:mobile_flutter/theme.dart';

/// 「医生代拍」交付结果弹窗:记录数说明 + 可复制的口令 + 「分享文件」按钮。
///
/// 与 `screens/export_screen.dart` 的 `_showShareResult` 是**同一份 UI/文案的
/// 独立副本**,不是共享组件——两者交付方向相反(患者→医生 vs 医生→病人),
/// 措辞不能共用同一句话;拆开维护也让「不碰患者/家属模式一行代码」这条硬规矩
/// 在这个文件上是显而易见成立的(`export_screen.dart` 完全不知道本文件存在)。
/// 宁可这 ~100 行重复,也不去改 `export_screen.dart` 抽公共组件。
Future<void> showDoctorShareResultDialog(
  BuildContext context,
  ShareResultDto result, {
  required Rect Function() shareOrigin,
}) async {
  if (!context.mounted) return;
  await showDialog<void>(
    context: context,
    builder: (context) => AlertDialog(
      title: const Text('加密文件已生成'),
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
            '请把这份文件交给病人本人保管,口令请当面口头告知或用不同渠道另发;'
            '对方打开文件、输入口令即可查看,数据始终端到端加密。这台设备不会留底。',
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
                    await Clipboard.setData(ClipboardData(text: result.passphrase));
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
          style: FilledButton.styleFrom(backgroundColor: MedMe.proxyOrange),
          icon: const Icon(Icons.ios_share),
          label: const Text('分享文件'),
          onPressed: () async {
            await SharePlus.instance.share(
              ShareParams(
                files: [XFile(result.path)],
                subject: 'MedMe 病历(代建档)',
                sharePositionOrigin: shareOrigin(),
              ),
            );
          },
        ),
      ],
    ),
  );
}
