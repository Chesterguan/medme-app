// 化验单结构化解析(纯 Dart,便于单测)—— 移植自桌面端
// apps/desktop/src/labTable.ts,解决同一个根因:文本提取/OCR 会把化验行原本
// 靠多空格对齐的列全部折叠成单空格,原本靠"两个以上空格/Tab"分列的化验行
// 退化成一整行散文本,无法再按列切分。这里改用"结构"而非"空白宽度"来切列:
//   值(value) = 第一个独立的数字 token(如 6.05 / 7.1 / 95)
//   单位(unit) = 紧跟在值后面的 token,前提是它不像参考范围的起始(否则视为无单位)
//   项目名(name) = 值之前的所有 token
//   参考范围/提示(range) = 单位之后的所有 token(如 "< 5.20 ↑" / "3.9 - 6.1 ↑")
// 对形如 "TC 总胆固醇 Cholesterol 6.05 mmol/L < 5.20 ↑" 的单空格行同样适用。

/// 化验行的异常提示。
enum LabFlag { high, low, normal }

/// 一行化验结果:项目名 / 结果值 / 单位 / 参考范围(含提示符号)。
class LabRow {
  final String name;
  final String value;
  final String unit;
  final String range;
  final LabFlag? flag;

  const LabRow({
    required this.name,
    required this.value,
    required this.unit,
    required this.range,
    required this.flag,
  });

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      (other is LabRow &&
          other.name == name &&
          other.value == value &&
          other.unit == unit &&
          other.range == range &&
          other.flag == flag);

  @override
  int get hashCode => Object.hash(name, value, unit, range, flag);

  @override
  String toString() =>
      'LabRow(name: $name, value: $value, unit: $unit, range: $range, flag: $flag)';
}

final RegExp _numRe = RegExp(r'^-?\d+(\.\d+)?$');
final RegExp _rangeStartRe = RegExp(r'^[<>≤≥]');

LabFlag? _labFlag(String range) {
  if (RegExp(r'↑|偏高|升高').hasMatch(range)) return LabFlag.high;
  if (RegExp(r'↓|偏低|降低|减低').hasMatch(range)) return LabFlag.low;
  if (range.contains('正常')) return LabFlag.normal;
  return null;
}

/// 把一行拆成结构化的化验行;不像化验行(找不到独立数值,或数值前/后缺内容)
/// 则返回 null。
LabRow? parseLabRow(String line) {
  final tokens = line
      .trim()
      .split(RegExp(r'\s+'))
      .where((t) => t.isNotEmpty)
      .toList();
  if (tokens.length < 2) return null;

  final valueIdx = tokens.indexWhere((t) => _numRe.hasMatch(t));
  if (valueIdx <= 0) return null; // 值前必须有项目名;值本身不能是第一个 token

  final after = tokens.sublist(valueIdx + 1);
  if (after.isEmpty) return null; // 值后至少要有单位或参考范围,否则不像化验行

  final name = tokens.sublist(0, valueIdx).join(' ');
  final value = tokens[valueIdx];

  // 判断值后第一个 token 是参考范围的开头(比较符/数字/连字符),还是单位。
  final nextTok = after[0];
  final looksLikeRangeStart =
      _rangeStartRe.hasMatch(nextTok) ||
      _numRe.hasMatch(nextTok) ||
      nextTok == '-';
  final unit = looksLikeRangeStart ? '' : nextTok;
  final rangeTokens = looksLikeRangeStart ? after : after.sublist(1);
  final range = rangeTokens.join(' ');

  return LabRow(
    name: name,
    value: value,
    unit: unit,
    range: range,
    flag: _labFlag(range),
  );
}

/// 化验表表头行(如"项目缩写 项目名称 结果 单位 参考范围 提示")—— 只用于识别
/// 并消费掉表头,不做数据解析(表头本身不含数字,parseLabRow 对其恒返回 null)。
bool isLabHeaderLine(String line) {
  const keys = ['项目', '结果', '单位', '参考', '提示', '名称', '缩写'];
  final hits = keys.where((k) => line.contains(k)).length;
  return hits >= 2 && !RegExp(r'\d').hasMatch(line);
}

/// 连续 ≥3 行都能解析成化验行,才判定为化验表(避免把偶尔带数字的普通段落误判)。
const int minLabRows = 3;

/// 一段被识别出的连续化验表:解析出的行 + 下一个未消费行号(含表头在内)。
class LabRun {
  final List<LabRow> rows;

  /// 下一个未消费的行号(含表头在内)。
  final int next;

  const LabRun({required this.rows, required this.next});
}

// 只跳过恰好一行空行(实测文本提取/OCR 会在提取出的每一"行"之间都插入一个
// 空行——不是段落间距,是逐行现象;连续 ≥2 行空行更像是真正的段落分隔,
// 不再当表格处理)。
int _skipSingleBlank(List<String> lines, int k) {
  return k < lines.length && lines[k].trim().isEmpty ? k + 1 : k;
}

/// 从 lines[i] 开始尝试识别一段连续的化验表:可选的表头行 + ≥3 行连续化验
/// 数据行(行间允许被逐行插入的单个空行打断,见 [_skipSingleBlank])。
/// 识别失败(不足 3 行连续化验行)返回 null,调用方应回退到通用解析。
LabRun? tryParseLabRun(List<String> lines, int i) {
  final trimmed = lines[i].trim();
  final start = isLabHeaderLine(trimmed) ? _skipSingleBlank(lines, i + 1) : i;

  final rows = <LabRow>[];
  var j = start;
  while (j < lines.length) {
    final l = lines[j].trim();
    final row = l.isNotEmpty ? parseLabRow(l) : null;
    if (row == null) break;
    rows.add(row);
    j = _skipSingleBlank(lines, j + 1);
  }

  if (rows.length < minLabRows) return null;
  return LabRun(rows: rows, next: j);
}

/// 便捷入口:给定一段(疑似)化验行文本,从头按结构解析出所有连续可识别的
/// 化验行(可选前置表头,行间容忍单个空行);不足 3 行连续则视为不是化验表,
/// 返回空列表 —— 调用方应回退到通用文本渲染。
List<LabRow> parseLabRows(List<String> lines) {
  if (lines.isEmpty) return const [];
  final run = tryParseLabRun(lines, 0);
  return run?.rows ?? const [];
}
