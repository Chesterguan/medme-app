import 'package:flutter/material.dart';

/// 底部导航一级 tab「设置」—— 载入示例数据 / 清空重置 / iCloud 同步 / 加密分享入口 / 关于。
/// (导出已移到「导入导出」tab,不再放这里。)P3/P5 填充;此处占位。
class SettingsScreen extends StatelessWidget {
  const SettingsScreen({super.key});

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('设置')),
      body: const Center(child: Text('设置(P3/P5 实现)')),
    );
  }
}
