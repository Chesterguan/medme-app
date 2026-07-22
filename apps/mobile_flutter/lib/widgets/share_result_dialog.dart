import 'package:flutter/material.dart';
import 'package:flutter/services.dart' show Clipboard, ClipboardData;
import 'package:share_plus/share_plus.dart';

import 'package:mobile_flutter/src/rust/api/dto.dart';
import 'package:mobile_flutter/theme.dart';

/// 加密分享结果弹窗:记录数说明 + 可复制的口令 + 「分享文件」按钮。正常导出分享
/// (`screens/export_screen.dart`)与医生代拍临时会话(`screens/proxy/proxy_intake_flow.dart`)
/// 共用同一份 UI/文案——都是同一套 `ShareResultDto`(文件 + 口令),别复制。
///
/// [shareOrigin] 供 iOS(尤其 iPad)系统分享面板定位 popover 锚点(见
/// `SharePlus` 的 `sharePositionOrigin`)。
Future<void> showShareResultDialog(
  BuildContext context,
  ShareResultDto result, {
  required Rect Function() shareOrigin,
  String shareSubject = 'MedMe 加密病历',
  // 正常导出分享是「患者 → 医生」,医生代拍临时会话是「医生 → 病人」——交付方向
  // 相反,措辞不能共用同一句话,故留一个可覆盖的提示文案(默认沿用正常导出分享
  // 原文案,逐字不变)。
  String deliveryHint = '把文件发给医生,口令请用不同渠道另发(比如短信、电话告知);'
      '对方打开文件、输入口令即可查看,数据始终端到端加密。',
}) async {
  if (!context.mounted) return;
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
          Text(
            deliveryHint,
            style: const TextStyle(fontSize: 13.5, color: MedMe.faint, height: 1.5),
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
          icon: const Icon(Icons.ios_share),
          label: const Text('分享文件'),
          onPressed: () async {
            await SharePlus.instance.share(
              ShareParams(
                files: [XFile(result.path)],
                subject: shareSubject,
                sharePositionOrigin: shareOrigin(),
              ),
            );
          },
        ),
      ],
    ),
  );
}
