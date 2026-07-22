import 'package:path_provider/path_provider.dart';

import 'package:mobile_flutter/src/rust/api/dto.dart';
import 'package:mobile_flutter/src/rust/api/ephemeral.dart' as rust_ephemeral;

/// 「为病人代建档」临时会话(即焚)的 Dart 侧薄封装 —— 直接转发到 Rust 侧独立的
/// `EPHEMERAL` cell(`api::ephemeral`,与医生自己的 `VAULT` cell 完全平行、互不
/// 可见)。**不碰** `ProfileManager` / `vault_boot` 的任何 reopen/switch:这不是
/// 「切成员」,是另一个用完即焚的箱子。
class EphemeralSession {
  EphemeralSession._();

  /// 会话根目录:iOS/安卓的临时缓存目录(不进 iCloud/云备份,系统可能随时清空——
  /// 与「用完即焚」互为兜底)。
  static Future<String> _cacheDir() async =>
      (await getTemporaryDirectory()).path;

  /// 启动时清崩溃残留。`main()` 里 `RustLib.init()` 之后调一次,不依赖是否曾开过
  /// 会话。
  static Future<void> sweep() async {
    await rust_ephemeral.ephemeralSweep(cacheDir: await _cacheDir());
  }

  /// 开始一次新会话:全新空箱,一次性随机设备 id(不落盘、不带医生设备身份)。
  static Future<void> begin() async {
    await rust_ephemeral.ephemeralBegin(cacheDir: await _cacheDir());
  }

  /// 采集(图片,已识别好文本)。签名与 `vault.dart` 的 `ingestImageWithText` 一致,
  /// 供 `ingestPendingItems`(`import_flow.dart`)直接注入复用。
  static Future<ImportOutcomeDto> ingestImageWithText({
    required String name,
    required List<int> bytes,
    required String ocrText,
    required double confidence,
  }) => rust_ephemeral.ephemeralIngestImageWithText(
    name: name,
    bytes: bytes,
    ocrText: ocrText,
    confidence: confidence,
  );

  /// 采集(字节直传)。签名与 `vault.dart` 的 `ingestBytes` 一致。
  static Future<ImportOutcomeDto> ingestBytes({
    required String filename,
    required List<int> data,
  }) => rust_ephemeral.ephemeralIngestBytes(filename: filename, data: data);

  /// 预览时间线:交付前让医生核对这次代拍收了什么、分类对不对。
  static Future<List<TimelineGroupDto>> loadPreview() =>
      rust_ephemeral.ephemeralLoadPreview();

  /// 打包成加密分享件(带拍前同意记录),写进**临时会话箱**——不是医生自己的档案。
  static Future<ShareResultDto> createShare({
    required int expiresDays,
    required ConsentDto consent,
  }) => rust_ephemeral.ephemeralCreateShare(
    expiresDays: expiresDays,
    consent: consent,
  );

  /// 即焚:关闭 db/日志句柄 + 整棵删掉会话目录(原始字节/事件日志/OCR 文本/
  /// 生成的分享文件全在里面)。幂等——未开始过会话时是 no-op。「尽力」语义:
  /// 失败不应阻塞 UI(用户已经拿到文件或已经取消了),Rust 侧本身也是
  /// best-effort `remove_dir_all`,这里再兜一层不让异常上抛打断收尾流程。
  static Future<void> wipe() async {
    try {
      await rust_ephemeral.ephemeralWipe();
    } catch (_) {
      // 忽略:见上方文档。
    }
  }
}
