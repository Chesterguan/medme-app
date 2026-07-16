import 'package:flutter/services.dart';

/// iCloud 原生桥(iOS-only,MethodChannel `medme/icloud`)。解析 ubiquity 容器路径,
/// 由 Dart 拿到后作参数传给 Rust FFI(`openVault` / `enableIcloudSync`)。
///
/// 为什么走 channel 而不是 Rust 直接调 Swift:FRB/cargokit 把 Rust 编成独立
/// framework,框架不能反向链接 app target 里的 Swift `@_cdecl` 符号(archive linker
/// 会失败)。MethodChannel handler 住在 app target(AppDelegate),没有这个限制。
class IcloudBridge {
  static const _channel = MethodChannel('medme/icloud');

  /// iCloud 容器根路径;iCloud 不可用/未登录/非 iOS 返回 `null`。安卓等平台上 channel
  /// 没有 handler,`invokeMethod` 抛 `MissingPluginException`,这里捕获成 `null`。
  static Future<String?> containerPath() async {
    try {
      final path = await _channel.invokeMethod<String>('containerPath');
      return (path != null && path.isNotEmpty) ? path : null;
    } catch (_) {
      return null;
    }
  }

  /// iCloud 是否可用(容器能解析)。
  static Future<bool> available() async => (await containerPath()) != null;

  /// 确保 vault 里一份对象文件已从 iCloud 下载到本地。开启 iCloud 同步后,
  /// `objects/` 里的对象可能被 iCloud 逐出(只剩 `.icloud` 占位符);「查看原件」
  /// 读盘前先调它触发按需下载并等待落地(见 `AppDelegate.swift` 的 `ensureDownloaded`)。
  ///
  /// iOS-only。其它平台无 handler,`invokeMethod` 抛 `MissingPluginException`;文件已
  /// 是最新 / 非 iCloud 托管时原生侧立即返回。任何异常都按「就绪」放行(返回 true),
  /// 让上层照常读盘并在真失败时优雅降级,绝不因下载判断而卡住查看。
  static Future<bool> ensureDownloaded(String absolutePath) async {
    try {
      final ok = await _channel.invokeMethod<bool>('ensureDownloaded', {
        'path': absolutePath,
      });
      return ok ?? true;
    } catch (_) {
      return true;
    }
  }
}
