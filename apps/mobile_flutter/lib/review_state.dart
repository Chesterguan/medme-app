import 'dart:convert';
import 'dart:io';

import 'package:path_provider/path_provider.dart';

import 'package:mobile_flutter/profile_manager.dart';

/// 「新导入待确认」本地状态,**按成员分命名空间**(每个成员独立的待确认集,不同
/// 成员的保险箱各自从 id 1 起,共用一个集合会撞车)。持久化到沙盒
/// `<support>/review_state.json`(纯本设备 UI 状态,不进保险箱)。
///
/// 除了待确认集,还记录每份新导入报告里**识别到的患者姓名**——若它和当前成员档案
/// 名字不一致([_flagged]),说明可能导错了人,健康档案会给这份标红警告。
///
/// 语义:导入时把本次新建文档 id 显式加入**当前成员**的待确认集([markPending]);
/// 健康档案顶部把当前成员待确认集里的文档置顶让用户核对;点「确认」移除([markReviewed])。
class ReviewState {
  ReviewState._();
  static final ReviewState instance = ReviewState._();

  final Map<String, Set<int>> _byMember = {};
  // 成员 → (文档 id → 报告里识别到的、与该成员名字**不符**的患者姓名)。
  final Map<String, Map<int, String>> _flagged = {};
  bool _loaded = false;
  File? _file;

  Set<int> _cur() =>
      _byMember.putIfAbsent(ProfileManager.instance.current, () => <int>{});
  Map<int, String> _curFlagged() =>
      _flagged.putIfAbsent(ProfileManager.instance.current, () => <int, String>{});

  Future<File> _stateFile() async {
    if (_file != null) return _file!;
    final dir = await getApplicationSupportDirectory();
    return _file = File('${dir.path}/review_state.json');
  }

  Future<void> ensureLoaded() async {
    if (_loaded) return;
    try {
      final f = await _stateFile();
      if (await f.exists()) {
        final json = jsonDecode(await f.readAsString()) as Map<String, dynamic>;
        final pending = json['pending'] as Map<String, dynamic>?;
        if (pending != null) {
          _byMember.clear();
          pending.forEach((member, ids) {
            _byMember[member] = (ids as List).map((e) => e as int).toSet();
          });
        }
        final flagged = json['flagged'] as Map<String, dynamic>?;
        if (flagged != null) {
          _flagged.clear();
          flagged.forEach((member, m) {
            _flagged[member] = (m as Map<String, dynamic>).map(
              (k, v) => MapEntry(int.parse(k), v as String),
            );
          });
        }
      }
    } catch (_) {
      // 读坏了不致命:当空,后续导入会重填。
    }
    _loaded = true;
  }

  Future<void> _save() async {
    try {
      final f = await _stateFile();
      final pending = <String, List<int>>{};
      _byMember.forEach((m, ids) {
        if (ids.isNotEmpty) pending[m] = ids.toList();
      });
      final flagged = <String, Map<String, String>>{};
      _flagged.forEach((m, map) {
        if (map.isNotEmpty) {
          flagged[m] = map.map((k, v) => MapEntry(k.toString(), v));
        }
      });
      await f.writeAsString(jsonEncode({'pending': pending, 'flagged': flagged}));
    } catch (_) {}
  }

  /// 当前成员下,该文档是否「新导入·待确认」。
  bool isPending(int docId) => _cur().contains(docId);

  /// 该待确认文档识别到的、与当前成员名字不符的患者姓名;一致或无则 null。
  String? mismatchName(int docId) => _curFlagged()[docId];

  /// 导入后把新建文档加入当前成员的待确认集。`docs` = 文档 id → 报告里识别到的
  /// 患者姓名(识别不到为 null);姓名与当前成员不符的记为「疑似导错人」。
  Future<void> markPending(Map<int, String?> docs) async {
    await ensureLoaded();
    final member = ProfileManager.instance.current;
    var changed = false;
    docs.forEach((id, detected) {
      changed = _cur().add(id) || changed;
      if (detected != null && detected.trim().isNotEmpty && detected != member) {
        _curFlagged()[id] = detected;
        changed = true;
      }
    });
    if (changed) await _save();
  }

  /// 确认通过一份 → 移出当前成员待确认集(连同标红)。
  Future<void> markReviewed(int docId) async {
    await ensureLoaded();
    final a = _cur().remove(docId);
    final b = _curFlagged().remove(docId) != null;
    if (a || b) await _save();
  }

  /// 一键全部确认(当前成员)。
  Future<void> markAllReviewed(Iterable<int> docIds) async {
    await ensureLoaded();
    var changed = false;
    for (final id in docIds) {
      changed = _cur().remove(id) || changed;
      changed = (_curFlagged().remove(id) != null) || changed;
    }
    if (changed) await _save();
  }

  /// 清空全部成员的待确认/标红状态(「清空所有数据」恢复出厂时调)。
  Future<void> clearAll() async {
    await ensureLoaded();
    _byMember.clear();
    _flagged.clear();
    await _save();
  }

  /// 成员改名(如默认档案被识别到的姓名自动重命名)时迁移其待确认/标红键。
  Future<void> renameMember(String from, String to) async {
    if (from == to) return;
    await ensureLoaded();
    final p = _byMember.remove(from);
    if (p != null) (_byMember[to] ??= <int>{}).addAll(p);
    final fl = _flagged.remove(from);
    if (fl != null) (_flagged[to] ??= <int, String>{}).addAll(fl);
    if (p != null || fl != null) await _save();
  }
}
