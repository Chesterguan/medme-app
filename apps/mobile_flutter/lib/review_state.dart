import 'dart:convert';
import 'dart:io';

import 'package:path_provider/path_provider.dart';

/// 「新导入待确认」本地状态。持久化一份「待确认文档 id 集」到沙盒
/// `<support>/review_state.json`(纯本设备 UI 状态,不进保险箱、不需跨设备同步)。
///
/// 语义(显式、可靠):
/// - **导入**时,把本次新建的文档 id 显式加入待确认集([markPending])。
/// - 健康档案顶部把待确认集里的文档列为「待确认」,置顶让用户过一眼。
/// - 用户点「确认」→ 从集里移除([markReviewed]),归入正常时间线。
///
/// 不再靠「基线/已审集」反向推断(那种做法对已有数据、去重、时序都容易出错——用户
/// 反馈过看不到待确认)。现在只有真正新导入的文档才进队列,零歧义、零 spine 风险。
class ReviewState {
  ReviewState._();
  static final ReviewState instance = ReviewState._();

  final Set<int> _pending = {};
  bool _loaded = false;
  File? _file;

  Future<File> _stateFile() async {
    if (_file != null) return _file!;
    final dir = await getApplicationSupportDirectory();
    return _file = File('${dir.path}/review_state.json');
  }

  /// 从磁盘载入待确认集(幂等,每会话读一次)。健康档案 build 前调一次。
  Future<void> ensureLoaded() async {
    if (_loaded) return;
    try {
      final f = await _stateFile();
      if (await f.exists()) {
        final json = jsonDecode(await f.readAsString()) as Map<String, dynamic>;
        _pending
          ..clear()
          ..addAll((json['pending'] as List? ?? const []).map((e) => e as int));
      }
    } catch (_) {
      // 读坏了不致命:当空集,后续导入会重新填。
    }
    _loaded = true;
  }

  Future<void> _save() async {
    try {
      final f = await _stateFile();
      await f.writeAsString(jsonEncode({'pending': _pending.toList()}));
    } catch (_) {
      // 写失败不致命(下次再写);本会话内存状态仍对。
    }
  }

  /// 该文档是否「新导入·待确认」。
  bool isPending(int docId) => _pending.contains(docId);

  /// 导入后把新建文档 id 加入待确认集。
  Future<void> markPending(Iterable<int> docIds) async {
    await ensureLoaded();
    var changed = false;
    for (final id in docIds) {
      changed = _pending.add(id) || changed;
    }
    if (changed) await _save();
  }

  /// 确认通过一份 → 移出待确认集(归入正常时间线)。
  Future<void> markReviewed(int docId) async {
    await ensureLoaded();
    if (_pending.remove(docId)) await _save();
  }

  /// 一键全部确认。
  Future<void> markAllReviewed(Iterable<int> docIds) async {
    await ensureLoaded();
    var changed = false;
    for (final id in docIds) {
      changed = _pending.remove(id) || changed;
    }
    if (changed) await _save();
  }
}
