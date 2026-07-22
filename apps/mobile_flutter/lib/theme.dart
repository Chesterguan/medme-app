import 'package:flutter/material.dart';

/// MedMe 医我 设计令牌 —— 与桌面 / 现有移动端(App.css)一致:teal 品牌色、
/// 柔和背景、圆角卡片。P3 各屏统一从这里取样式,别散落硬编码。
class MedMe {
  MedMe._();

  // 品牌
  static const Color teal = Color(0xFF1789C1);
  static const Color tealDark = Color(0xFF1560A8);
  static const Color tealSoft = Color(0xFFE6F6FA);

  // 中性
  static const Color bg = Color(0xFFF6F8FB);
  static const Color panel = Colors.white;
  static const Color line = Color(0xFFE2E8F0);
  static const Color ink = Color(0xFF1E293B);
  static const Color faint = Color(0xFF94A3B8);
  static const Color danger = Color(0xFFBE123C);

  // 「为病人代建档」全屏流程专用的警示橙 chrome —— 与品牌 teal 明显区分,
  // 让医生一眼分清「这不是我的箱」(见 `screens/proxy/proxy_intake_flow.dart`)。
  static const Color proxyOrange = Color(0xFFC2570C);
  static const Color proxyOrangeSoft = Color(0xFFFCEEE0);

  static ThemeData theme() {
    final scheme = ColorScheme.fromSeed(
      seedColor: teal,
      primary: teal,
      surface: panel,
    );
    return ThemeData(
      useMaterial3: true,
      colorScheme: scheme,
      scaffoldBackgroundColor: bg,
      fontFamily: 'PingFang SC',
      appBarTheme: const AppBarTheme(
        backgroundColor: panel,
        foregroundColor: ink,
        elevation: 0,
        scrolledUnderElevation: 0.5,
        centerTitle: false,
      ),
      navigationBarTheme: NavigationBarThemeData(
        backgroundColor: panel,
        indicatorColor: tealSoft,
        elevation: 3,
        labelTextStyle: WidgetStateProperty.all(
          const TextStyle(fontSize: 12, fontWeight: FontWeight.w600),
        ),
      ),
      cardTheme: CardThemeData(
        color: panel,
        elevation: 0,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(14),
          side: const BorderSide(color: line),
        ),
        margin: EdgeInsets.zero,
      ),
    );
  }
}
