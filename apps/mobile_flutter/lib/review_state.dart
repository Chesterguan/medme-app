import 'dart:convert';
import 'dart:io';

import 'package:path_provider/path_provider.dart';

import 'package:mobile_flutter/profile_manager.dart';

/// 「新导入待确认」本地状态,**按成员分命名空间**(每个成员独立的待确认集,不同
/// 成员的保险箱各自从 id 1 起,共用一个集合会撞车)。持久化到沙盒
/// `<support>/review_state.json`(纯本设备 UI 状态,不进保险箱)。
///
/// 语义:导入时把本次新建文档 id 显式加入**当前成员**的待确认集([markPending]);
/// 健康档案顶部把当前成员待确认集里的文档置顶让用户核对;点「确认」移除([markReviewed])。
class ReviewState {
  ReviewState._();
  static final ReviewState instance = ReviewState._();

  final Map<String, Set<int>> _byMember = {};
  bool _loaded = false;
  File? _file;

  Set<int> _cur() =>
      _byMember.putIfAbsent(ProfileManager.instance.current, () => <int>{});

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
      }
    } catch (_) {
      // 读坏了不致命:当空,后续导入会重填。
    }
    _loaded = true;
  }

  Future<void> _save() async {
    try {
      final f = await _stateFile();
      final map = <String, List<int>>{};
      _byMember.forEach((m, ids) {
        if (ids.isNotEmpty) map[m] = ids.toList();
      });
      await f.writeAsString(jsonEncode({'pending': map}));
    } catch (_) {}
  }

  /// 当前成员下,该文档是否「新导入·待确认」。
  bool isPending(int docId) => _cur().contains(docId);

  /// 导入后把新建文档 id 加入当前成员的待确认集。
  Future<void> markPending(Iterable<int> docIds) async {
    await ensureLoaded();
    var changed = false;
    for (final id in docIds) {
      changed = _cur().add(id) || changed;
    }
    if (changed) await _save();
  }

  /// 确认通过一份 → 移出当前成员待确认集。
  Future<void> markReviewed(int docId) async {
    await ensureLoaded();
    if (_cur().remove(docId)) await _save();
  }

  /// 一键全部确认(当前成员)。
  Future<void> markAllReviewed(Iterable<int> docIds) async {
    await ensureLoaded();
    var changed = false;
    for (final id in docIds) {
      changed = _cur().remove(id) || changed;
    }
    if (changed) await _save();
  }
}
