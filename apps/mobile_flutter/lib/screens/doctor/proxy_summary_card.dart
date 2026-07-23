import 'package:flutter/material.dart';

import 'package:mobile_flutter/src/rust/api/dto.dart';
import 'package:mobile_flutter/theme.dart';

/// 「病情摘要卡」(审阅屏选项 b 的核心):在治的病 + 关键化验 + 在用药,三十秒看懂
/// 这次代拍收上来的大局。数据来自 `EphemeralSession.summary()`(`ProxySummaryDto`,
/// Rust 侧复用与「生成加密分享」同一套 `parser::assemble_summary` 装配)。措辞与信息
/// 层级参考 `qr_share_screen.dart`「医生看到的是什么」一段:在治的疾病 · 关键化验的
/// 近期趋势 · 正在吃的药。
///
/// 干净的原生卡片:每个问题一块,内嵌它的化验(项目/最近值/趋势箭头)与在用药
/// (chips),不做图表——「清楚够用就行」。没有任何结构化问题时不占地方(原文仍在
/// 审阅屏下方「逐份识别内容」区块完整展开,不丢信息)。
class ProxySummaryCard extends StatelessWidget {
  const ProxySummaryCard({super.key, required this.summary});

  final ProxySummaryDto summary;

  @override
  Widget build(BuildContext context) {
    if (summary.problems.isEmpty) return const SizedBox.shrink();

    return Container(
      margin: const EdgeInsets.fromLTRB(16, 0, 16, 12),
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: MedMe.panel,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: MedMe.line),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text(
            '病情摘要',
            style: TextStyle(fontSize: 15, fontWeight: FontWeight.w800),
          ),
          const SizedBox(height: 2),
          const Text(
            '在治的病、关键化验、正在吃的药 —— 给医生三十秒看懂大局',
            style: TextStyle(fontSize: 12, color: MedMe.faint),
          ),
          for (final p in summary.problems) ...[
            const SizedBox(height: 14),
            _ProblemBlock(problem: p),
          ],
        ],
      ),
    );
  }
}

class _ProblemBlock extends StatelessWidget {
  const _ProblemBlock({required this.problem});

  final ProxyProblemDto problem;

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _StatusChip(term: problem.term, status: problem.status, warn: problem.warn),
        if (problem.labs.isNotEmpty) ...[
          const SizedBox(height: 8),
          for (final l in problem.labs) _LabRow(lab: l),
        ],
        if (problem.meds.isNotEmpty) ...[
          const SizedBox(height: 8),
          Wrap(
            spacing: 6,
            runSpacing: 6,
            children: [for (final m in problem.meds) _MedChip(med: m)],
          ),
        ],
      ],
    );
  }
}

class _StatusChip extends StatelessWidget {
  const _StatusChip({required this.term, required this.status, required this.warn});

  final String term;
  final String status;
  final bool warn;

  @override
  Widget build(BuildContext context) {
    final Color fg = warn ? MedMe.danger : MedMe.tealDark;
    final Color bg = warn ? const Color(0xFFFDECEF) : MedMe.tealSoft;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
      decoration: BoxDecoration(color: bg, borderRadius: BorderRadius.circular(999)),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(
            term,
            style: TextStyle(fontSize: 13.5, fontWeight: FontWeight.w700, color: fg),
          ),
          const SizedBox(width: 6),
          Text(
            status,
            style: TextStyle(fontSize: 11, fontWeight: FontWeight.w600, color: fg),
          ),
        ],
      ),
    );
  }
}

// 化验行:项目 最近值 单位 ↑/↓/→。异常沿用 `widgets/report_content.dart` 同一套
// H=amber-700/L=blue-700 配色(未导出为公共 API,故在此复述这两个色值,不改那个
// 文件)——保持全 app 化验异常配色一致,不另发明一套颜色语义。
const _labHighColor = Color(0xFFB45309);
const _labLowColor = Color(0xFF1D4ED8);

class _LabRow extends StatelessWidget {
  const _LabRow({required this.lab});

  final ProxyLabDto lab;

  @override
  Widget build(BuildContext context) {
    final abnormalHigh = lab.refHigh != null && lab.latestValue > lab.refHigh!;
    final abnormalLow = lab.refLow != null && lab.latestValue < lab.refLow!;
    final color = abnormalHigh
        ? _labHighColor
        : abnormalLow
        ? _labLowColor
        : MedMe.ink;
    final value = _fmtValue(lab.latestValue);
    final unit = lab.unit ?? '';
    final arrow = switch (lab.trend) {
      'up' => '↑',
      'down' => '↓',
      'flat' => '→',
      _ => '',
    };
    return Padding(
      padding: const EdgeInsets.only(left: 4, bottom: 4),
      child: Row(
        children: [
          Expanded(
            child: Text(
              lab.name,
              style: const TextStyle(fontSize: 13, color: MedMe.ink),
              overflow: TextOverflow.ellipsis,
            ),
          ),
          Text(
            '$value$unit',
            style: TextStyle(fontSize: 13, fontWeight: FontWeight.w600, color: color),
          ),
          if (arrow.isNotEmpty) ...[
            const SizedBox(width: 4),
            Text(arrow, style: TextStyle(fontSize: 13, fontWeight: FontWeight.w700, color: color)),
          ],
        ],
      ),
    );
  }
}

String _fmtValue(double v) {
  // 整数值不带尾随 .0(88.0 → 88),与 `parser::aggregate::fmt_num` 同一惯例。
  return v == v.roundToDouble() ? v.toStringAsFixed(0) : v.toString();
}

class _MedChip extends StatelessWidget {
  const _MedChip({required this.med});

  final ProxyMedDto med;

  @override
  Widget build(BuildContext context) {
    final label = med.dose != null ? '${med.name} ${med.dose}' : med.name;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
      decoration: BoxDecoration(
        color: med.active ? const Color(0xFFECFDF5) : MedMe.bg,
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: med.active ? const Color(0xFFD1FAE5) : MedMe.line),
      ),
      child: Text(
        label,
        style: TextStyle(
          fontSize: 12.5,
          fontWeight: FontWeight.w600,
          color: med.active ? const Color(0xFF047857) : MedMe.faint,
        ),
      ),
    );
  }
}
