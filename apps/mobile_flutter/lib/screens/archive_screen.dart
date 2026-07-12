import 'package:flutter/material.dart';

/// 底部导航一级 tab「健康档案」—— 生命时间线:就诊组 + 独立文档,点开详情。
/// P3 填充(调 FFI load_archive / get_document + 内容感知渲染);此处占位。
class ArchiveScreen extends StatelessWidget {
  const ArchiveScreen({super.key});

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('健康档案')),
      body: const Center(child: Text('时间线(P3 实现)')),
    );
  }
}
