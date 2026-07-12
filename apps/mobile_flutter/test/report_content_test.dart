// 化验行结构化解析的单测。喂入文本提取/OCR 把所有空白折叠成单空格后的真实行
// (原始报告里各列靠多空格对齐,提取后退化成单空格分隔的散文本),验证仍能按
// 结构切出 项目名/结果/单位/参考范围+提示。镜像
// apps/desktop/test/labTable.test.ts 的断言。
import 'package:flutter_test/flutter_test.dart';
import 'package:mobile_flutter/report_content.dart';

// 血脂血糖化验单示例(单空格,来自文本提取的真实输出形态)。
const header = '项目缩写 项目名称 结果 单位 参考范围 提示';
const rows = [
  'TC 总胆固醇 Cholesterol 6.05 mmol/L < 5.20 ↑',
  'TG 甘油三酯 Triglyceride 2.35 mmol/L < 1.70 ↑',
  'HDL-C 高密度脂蛋白胆固醇 0.98 mmol/L > 1.04 ↓',
  'GLU 空腹血糖 Glucose 7.1 mmol/L 3.9 - 6.1 ↑',
  'HbA1c 糖化血红蛋白 6.9 % 4.0 - 6.0 ↑',
  'Cr 肌酐 Creatinine 95 umol/L 57 - 97 正常',
  'BUN 尿素氮 6.1 mmol/L 3.1 - 8.0 正常',
];

void main() {
  test('isLabHeaderLine 识别化验表表头,且表头本身不是数据行', () {
    expect(isLabHeaderLine(header), isTrue);
    expect(parseLabRow(header), isNull);
  });

  test('parseLabRow 把单空格行按结构切成 名称/值/单位/范围+提示', () {
    expect(
      parseLabRow(rows[0]),
      const LabRow(
        name: 'TC 总胆固醇 Cholesterol',
        value: '6.05',
        unit: 'mmol/L',
        range: '< 5.20 ↑',
        flag: LabFlag.high,
      ),
    );
    expect(
      parseLabRow(rows[1]),
      const LabRow(
        name: 'TG 甘油三酯 Triglyceride',
        value: '2.35',
        unit: 'mmol/L',
        range: '< 1.70 ↑',
        flag: LabFlag.high,
      ),
    );
    expect(
      parseLabRow(rows[2]),
      const LabRow(
        name: 'HDL-C 高密度脂蛋白胆固醇',
        value: '0.98',
        unit: 'mmol/L',
        range: '> 1.04 ↓',
        flag: LabFlag.low,
      ),
    );
    expect(
      parseLabRow(rows[3]),
      const LabRow(
        name: 'GLU 空腹血糖 Glucose',
        value: '7.1',
        unit: 'mmol/L',
        range: '3.9 - 6.1 ↑',
        flag: LabFlag.high,
      ),
    );
    expect(
      parseLabRow(rows[4]),
      const LabRow(
        name: 'HbA1c 糖化血红蛋白',
        value: '6.9',
        unit: '%',
        range: '4.0 - 6.0 ↑',
        flag: LabFlag.high,
      ),
    );
    expect(
      parseLabRow(rows[5]),
      const LabRow(
        name: 'Cr 肌酐 Creatinine',
        value: '95',
        unit: 'umol/L',
        range: '57 - 97 正常',
        flag: LabFlag.normal,
      ),
    );
    expect(
      parseLabRow(rows[6]),
      const LabRow(
        name: 'BUN 尿素氮',
        value: '6.1',
        unit: 'mmol/L',
        range: '3.1 - 8.0 正常',
        flag: LabFlag.normal,
      ),
    );
  });

  test('parseLabRow 对没有单位的行也能优雅处理(值后直接是参考范围)', () {
    final row = parseLabRow('WBC 白细胞计数 5.2 3.5 - 9.5 正常');
    expect(
      row,
      const LabRow(
        name: 'WBC 白细胞计数',
        value: '5.2',
        unit: '',
        range: '3.5 - 9.5 正常',
        flag: LabFlag.normal,
      ),
    );
  });

  test('parseLabRow 对普通段落(无独立数值,或数值打头)返回 null', () {
    expect(parseLabRow('患者近期体检未见明显异常。'), isNull);
    expect(parseLabRow('6.05 单独一个数字打头,前面没有项目名'), isNull);
    expect(parseLabRow('孤零零一个数字 6.05'), isNull); // 值后没有任何内容
  });

  test('parseLabRow 对患者信息行(数字打头/末尾无内容)不误判为化验行', () {
    // "45" 是末尾数字 token,值后没有单位/参考范围 —— 不像化验行。
    expect(parseLabRow('姓名 张三 性别 男 年龄 45'), isNull);
  });

  test('parseLabRow 对「提示:」类脚注(无独立数值)不误判为化验行', () {
    expect(parseLabRow('提示:血脂四项异常,建议内分泌科随诊。'), isNull);
  });

  test('tryParseLabRun 消费表头 + 连续 ≥3 行化验数据,整段判定为化验表', () {
    final lines = [header, ...rows, '', '备注:空腹采血。'];
    final run = tryParseLabRun(lines, 0);
    expect(run, isNotNull);
    expect(run!.rows.length, rows.length);
    expect(run.rows[0].name, 'TC 总胆固醇 Cholesterol');
    expect(lines[run.next].trim(), '备注:空腹采血。'); // 停在化验表之后的下一段落
  });

  test('tryParseLabRun 容忍化验行之间夹杂的单个空行(真实文本提取输出形态)', () {
    final lines = [
      header,
      '',
      rows[0],
      '',
      rows[1],
      '',
      rows[2],
      '',
      rows[3],
      '',
      '提示:血脂四项异常,建议内分泌科随诊。',
      '',
    ];
    final run = tryParseLabRun(lines, 0);
    expect(run, isNotNull);
    expect(run!.rows.length, 4);
    expect(run.rows[0].name, 'TC 总胆固醇 Cholesterol');
    expect(run.rows[3].name, 'GLU 空腹血糖 Glucose');
    expect(lines[run.next].trim(), '提示:血脂四项异常,建议内分泌科随诊。');
  });

  test('tryParseLabRun 连续 ≥2 行空行判定为真正的段落分隔,不再当作化验行间隔', () {
    final lines = [header, '', rows[0], '', '', rows[1], rows[2]];
    // TC 后面是两个空行 → 表格在此截断,只拿到 1 行,不足 3 行 → 判定失败
    expect(tryParseLabRun(lines, 0), isNull);
  });

  test('tryParseLabRun 不足 3 行连续化验行时判定失败,交回通用解析', () {
    final lines = [header, rows[0], rows[1], '以下省略,仅两行数据。'];
    expect(tryParseLabRun(lines, 0), isNull);
  });

  test('tryParseLabRun 没有表头也能从数据行直接起判', () {
    final run = tryParseLabRun(rows, 0);
    expect(run, isNotNull);
    expect(run!.rows.length, rows.length);
    expect(run.next, rows.length);
  });

  test('parseLabRows 从 表头+连续行 中解析出全部化验行', () {
    final result = parseLabRows([header, ...rows]);
    expect(result.length, rows.length);
    expect(result.first.name, 'TC 总胆固醇 Cholesterol');
    expect(result.last.name, 'BUN 尿素氮');
  });

  test('parseLabRows 对不足 3 行连续化验行的输入返回空列表(交回通用解析)', () {
    final result = parseLabRows([header, rows[0], rows[1], '以下省略,仅两行数据。']);
    expect(result, isEmpty);
  });
}
