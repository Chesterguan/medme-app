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
}
