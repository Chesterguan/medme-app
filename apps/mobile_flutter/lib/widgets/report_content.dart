import 'package:flutter/material.dart';

import '../report_content.dart' show LabFlag, LabRow, tryParseLabRun;
import '../theme.dart';

// 内容感知渲染(维度 B):按文档类型富渲染,移植自桌面端
// apps/desktop/src/components/ReportContent.tsx。
//  - 化验 → 表格(指标/结果/单位/参考范围,按 ↑/↓ 着色)
//  - 处方 → 用药清单卡片
//  - 病理/影像/出院/病历/手术 → 分节(【…】/结论/诊断 等标题加粗)+ 行内标签加粗
//  - 其余/解析不到结构 → 退回干净段落 —— 永不比原文更糟(见 memory:
//    content-aware-rendering)。

/// 档案/文档详情屏复用的富文本渲染;`docType` 为空或未知类型时退回通用分块。
class ReportContent extends StatelessWidget {
  final String text;
  final String? docType;

  const ReportContent({super.key, required this.text, this.docType});

  @override
  Widget build(BuildContext context) {
    if (text.trim().isEmpty) {
      return const Text(
        '无文本内容。',
        style: TextStyle(color: MedMe.faint, fontSize: 13),
      );
    }

    // 处方 → 用药清单
    if (docType == 'prescription') {
      final meds = _parseMeds(text);
      if (meds != null) {
        return _MedsView(meds: meds);
      }
    }

    // 其余类型(化验表格 / 病理·影像·出院·病历·手术 分节+行内标签 / 通用)
    final blocks = _parseBlocks(text);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        for (var i = 0; i < blocks.length; i++) ...[
          if (i > 0) const SizedBox(height: 16),
          _blockView(blocks[i]),
        ],
      ],
    );
  }
}

// ── 分块模型(化验表 / 通用多空格表 / 分节标题 / 段落)──

sealed class _Block {}

class _LabTableBlock extends _Block {
  final List<LabRow> rows;
  _LabTableBlock(this.rows);
}

class _TableBlock extends _Block {
  final List<String>? header;
  final List<List<String>> rows;
  _TableBlock(this.header, this.rows);
}

class _SectionBlock extends _Block {
  final String text;
  _SectionBlock(this.text);
}

class _ParaBlock extends _Block {
  final String text;
  _ParaBlock(this.text);
}

Widget _blockView(_Block b) {
  return switch (b) {
    _LabTableBlock(:final rows) => _LabTableView(rows: rows),
    _TableBlock(:final header, :final rows) => _GenericTableView(
      header: header,
      rows: rows,
    ),
    _SectionBlock(:final text) => _SectionView(text: text),
    _ParaBlock(:final text) => _ParaView(text: text),
  };
}

final RegExp _sectionBracketRe = RegExp(r'^[【\[].+[】\]]$');
final RegExp _shortLabelColonRe = RegExp(r'[:：]$');

List<_Block> _parseBlocks(String text) {
  final lines = text.split(RegExp(r'\r?\n'));
  final blocks = <_Block>[];
  var i = 0;
  while (i < lines.length) {
    final trimmed = lines[i].trim();
    if (trimmed.isEmpty) {
      i++;
      continue;
    }

    // 化验单单空格塌陷场景:先按结构尝试识别连续的化验行(见 ../report_content.dart),
    // 命中则优先于下面基于"多空格分列"的通用表格解析。
    final labRun = tryParseLabRun(lines, i);
    if (labRun != null) {
      blocks.add(_LabTableBlock(labRun.rows));
      i = labRun.next;
      continue;
    }

    if (_isTableHeader(trimmed) || _isDataRow(trimmed)) {
      final start = i;
      final header = _isTableHeader(trimmed) ? _splitCells(trimmed) : null;
      if (header != null) i++;
      final rows = <List<String>>[];
      while (i < lines.length &&
          lines[i].trim().isNotEmpty &&
          _isDataRow(lines[i])) {
        rows.add(_splitCells(lines[i]));
        i++;
      }
      if (rows.length >= 2) {
        blocks.add(_TableBlock(header, rows));
        continue;
      }
      i = start;
    }

    if (_sectionBracketRe.hasMatch(trimmed) ||
        (trimmed.length <= 14 && _shortLabelColonRe.hasMatch(trimmed))) {
      blocks.add(_SectionBlock(trimmed));
    } else {
      blocks.add(_ParaBlock(lines[i]));
    }
    i++;
  }
  return blocks;
}

List<String> _splitCells(String line) {
  return line
      .trim()
      .split(RegExp(r'\s{2,}|\t'))
      .where((c) => c.isNotEmpty)
      .toList();
}

bool _isTableHeader(String line) {
  const keys = ['项目', '结果', '单位', '参考', '提示', '名称', '缩写'];
  final hits = keys.where((k) => line.contains(k)).length;
  return hits >= 2 && _splitCells(line).length >= 3;
}

bool _isDataRow(String line) {
  return _splitCells(line).length >= 3 && RegExp(r'\d').hasMatch(line);
}

LabFlag? _rowStatus(List<String> cells) {
  final j = cells.join(' ');
  if (cells.contains('↑') || RegExp(r'↑|偏高|升高').hasMatch(j))
    return LabFlag.high;
  if (cells.contains('↓') || RegExp(r'↓|偏低|降低|减低').hasMatch(j))
    return LabFlag.low;
  if (j.contains('正常')) return LabFlag.normal;
  return null;
}

Color _flagColor(LabFlag? flag) {
  if (flag == LabFlag.high) return const Color(0xFFB45309); // amber-700
  if (flag == LabFlag.low) return const Color(0xFF1D4ED8); // blue-700
  return MedMe.ink;
}

// ── 段落:行内"标签:内容" → 标签加粗(主诉:/病理诊断:/诊断意见:…)──

final RegExp _labelRe = RegExp(r'^([一-龥A-Za-z]{2,10})([:：])(.*)$');

class _ParaView extends StatelessWidget {
  final String text;
  const _ParaView({required this.text});

  @override
  Widget build(BuildContext context) {
    const style = TextStyle(fontSize: 15, height: 1.6, color: MedMe.ink);
    final t = text.trimRight();
    final m = _labelRe.firstMatch(t);
    if (m != null && m.group(3)!.trim().isNotEmpty) {
      return RichText(
        text: TextSpan(
          style: style,
          children: [
            TextSpan(
              text: '${m.group(1)}${m.group(2)}',
              style: const TextStyle(
                fontWeight: FontWeight.w600,
                color: MedMe.ink,
              ),
            ),
            TextSpan(text: m.group(3)),
          ],
        ),
      );
    }
    return Text(text, style: style);
  }
}

class _SectionView extends StatelessWidget {
  final String text;
  const _SectionView({required this.text});

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(top: 4),
      child: Text(
        text,
        style: const TextStyle(
          fontWeight: FontWeight.w700,
          color: MedMe.ink,
          fontSize: 15,
        ),
      ),
    );
  }
}

// ── 表格:化验表(结构化解析)与通用多空格表共用的外框/单元格样式 ──

class _TableFrame extends StatelessWidget {
  final Widget child;
  const _TableFrame({required this.child});

  @override
  Widget build(BuildContext context) {
    return ClipRRect(
      borderRadius: BorderRadius.circular(12),
      child: Container(
        decoration: BoxDecoration(border: Border.all(color: MedMe.line)),
        child: child,
      ),
    );
  }
}

TableRow _headerRow(List<String> headers) {
  return TableRow(
    decoration: const BoxDecoration(color: MedMe.bg),
    children: [
      for (final h in headers)
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
          child: Text(
            h,
            style: const TextStyle(
              fontSize: 11,
              fontWeight: FontWeight.w600,
              color: MedMe.faint,
            ),
          ),
        ),
    ],
  );
}

Widget _cell(String text, Color color, {bool mono = false}) {
  return Padding(
    padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
    child: Text(
      text,
      style: TextStyle(
        fontSize: 13,
        color: color,
        fontFamily: mono ? 'monospace' : null,
      ),
    ),
  );
}

BoxDecoration? _zebra(int index) =>
    index.isOdd ? const BoxDecoration(color: Color(0x0A64748B)) : null;

class _LabTableView extends StatelessWidget {
  final List<LabRow> rows;
  const _LabTableView({required this.rows});

  static const _headers = ['项目', '结果', '单位', '参考范围/提示'];

  @override
  Widget build(BuildContext context) {
    return _TableFrame(
      child: Table(
        columnWidths: const {
          0: FlexColumnWidth(2.2),
          1: FlexColumnWidth(1),
          2: FlexColumnWidth(1),
          3: FlexColumnWidth(1.6),
        },
        defaultVerticalAlignment: TableCellVerticalAlignment.middle,
        children: [
          _headerRow(_headers),
          for (var i = 0; i < rows.length; i++) _dataRow(i, rows[i]),
        ],
      ),
    );
  }

  TableRow _dataRow(int index, LabRow r) {
    final color = _flagColor(r.flag);
    return TableRow(
      decoration: _zebra(index),
      children: [
        _cell(r.name, color),
        _cell(r.value, color, mono: true),
        _cell(r.unit, color, mono: true),
        _cell(r.range, color, mono: true),
      ],
    );
  }
}

class _GenericTableView extends StatelessWidget {
  final List<String>? header;
  final List<List<String>> rows;
  const _GenericTableView({required this.header, required this.rows});

  @override
  Widget build(BuildContext context) {
    final cols = [
      header?.length ?? 0,
      for (final r in rows) r.length,
    ].reduce((a, b) => a > b ? a : b);

    return _TableFrame(
      child: Table(
        defaultVerticalAlignment: TableCellVerticalAlignment.middle,
        children: [
          if (header != null)
            _headerRow([
              for (var c = 0; c < cols; c++)
                c < header!.length ? header![c] : '',
            ]),
          for (var i = 0; i < rows.length; i++) _dataRow(i, rows[i], cols),
        ],
      ),
    );
  }

  TableRow _dataRow(int index, List<String> r, int cols) {
    final color = _flagColor(_rowStatus(r));
    return TableRow(
      decoration: _zebra(index),
      children: [
        for (var c = 0; c < cols; c++)
          _cell(c < r.length ? r[c] : '', color, mono: true),
      ],
    );
  }
}

// ── 处方:用药清单(移植自桌面端 ReportContent.tsx 的 parseMeds)──

class _Med {
  final String name;
  final List<String> usage;
  const _Med({required this.name, required this.usage});
}

class _MedsResult {
  final List<String> intro;
  final List<_Med> meds;
  final List<String> footer;
  const _MedsResult({
    required this.intro,
    required this.meds,
    required this.footer,
  });
}

final RegExp _numberedRe = RegExp(r'^(\d+)\s*[.、)]\s*(.+)');
final RegExp _footerKeywordRe = RegExp(r'^(医师|药师|审核|备注|Rp\.?|处方)');
final RegExp _rpOnlyRe = RegExp(r'^Rp\.?$');

_MedsResult? _parseMeds(String text) {
  final lines = text.split(RegExp(r'\r?\n'));
  final meds = <_Med>[];
  final intro = <String>[];
  final footer = <String>[];
  String? curName;
  var usage = <String>[];
  var started = false;
  var ended = false;

  void pushCur() {
    if (curName != null) {
      meds.add(_Med(name: curName!, usage: usage));
      curName = null;
      usage = <String>[];
    }
  }

  for (final raw in lines) {
    final line = raw.trim();
    final numbered = _numberedRe.firstMatch(line);
    if (numbered != null) {
      started = true;
      ended = false;
      pushCur();
      curName = numbered.group(2)!.trim();
      continue;
    }
    if (_footerKeywordRe.hasMatch(line)) {
      pushCur();
      if (started) ended = true;
      if (line.isNotEmpty && !_rpOnlyRe.hasMatch(line)) {
        if (started) {
          footer.add(line);
        } else {
          intro.add(line);
        }
      }
      continue;
    }
    if (curName != null && line.isNotEmpty) {
      usage.add(line);
      continue;
    }
    if (line.isNotEmpty) {
      if (!started) {
        intro.add(line);
      } else if (ended) {
        footer.add(line);
      }
    }
  }
  pushCur();
  return meds.isNotEmpty
      ? _MedsResult(intro: intro, meds: meds, footer: footer)
      : null;
}

class _MedsView extends StatelessWidget {
  final _MedsResult meds;
  const _MedsView({required this.meds});

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (meds.intro.isNotEmpty) ...[
          for (final t in meds.intro) _ParaView(text: t),
          const SizedBox(height: 16),
        ],
        const Text(
          '用药',
          style: TextStyle(
            fontSize: 11,
            letterSpacing: 1.2,
            color: MedMe.faint,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: 8),
        for (var i = 0; i < meds.meds.length; i++) ...[
          if (i > 0) const SizedBox(height: 8),
          _MedCard(index: i, med: meds.meds[i]),
        ],
        if (meds.footer.isNotEmpty) ...[
          const SizedBox(height: 16),
          for (final t in meds.footer) _ParaView(text: t),
        ],
      ],
    );
  }
}

class _MedCard extends StatelessWidget {
  final int index;
  final _Med med;
  const _MedCard({required this.index, required this.med});

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: const Color(0xFFECFDF5), // emerald-50/40 近似
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: const Color(0xFFD1FAE5)), // emerald-100 近似
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Container(
            width: 28,
            height: 28,
            alignment: Alignment.center,
            decoration: BoxDecoration(
              color: const Color(0xFFD1FAE5),
              borderRadius: BorderRadius.circular(8),
            ),
            child: Text(
              '${index + 1}',
              style: const TextStyle(
                color: Color(0xFF047857),
                fontWeight: FontWeight.bold,
                fontSize: 14,
              ),
            ),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  med.name,
                  style: const TextStyle(
                    fontWeight: FontWeight.w500,
                    color: MedMe.ink,
                    fontSize: 15,
                  ),
                ),
                for (final u in med.usage)
                  Padding(
                    padding: const EdgeInsets.only(top: 2),
                    child: Text(
                      u,
                      style: const TextStyle(
                        fontSize: 13,
                        color: MedMe.faint,
                        height: 1.5,
                      ),
                    ),
                  ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}
