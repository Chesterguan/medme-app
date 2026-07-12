import 'package:flutter/material.dart';

/// 底部导航一级 tab「导入导出」(用户要求提升为一级,不埋设置)。
/// 采集/拍照/相册/文件导入 + 导出(时间线,后续带日期区间筛选)。
/// P3/P4/P6 填充实现;此处为占位骨架。
class ImportExportScreen extends StatelessWidget {
  const ImportExportScreen({super.key});

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('导入 · 导出')),
      body: const Center(child: Text('导入导出(P3/P4 实现)')),
    );
  }
}
