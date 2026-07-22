import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:path_provider/path_provider.dart';

/// 应用模式:「我管自己/家人的病历」(普通人)还是「我是医生,帮病人建档」
/// (医生代拍)。首次打开选一次,之后在「设置」可随时切换。
enum AppModeKind { personal, doctor }

/// 模式选择的持久化 + 全局信号。与 `ProfileManager`/`ReviewState` 同一约定:
/// 沙盒 support 目录下一个小 JSON 文件,不进保险箱本身(纯本机 UI 状态,与哪个
/// 成员/哪个保险箱无关)。
///
/// [mode] 为 `null` 表示「还没选过」——`main.dart` 的 `AppRoot` 据此决定是否先
/// 弹「你是?」选择屏。
class AppMode {
  AppMode._();
  static final AppMode instance = AppMode._();

  final ValueNotifier<AppModeKind?> mode = ValueNotifier<AppModeKind?>(null);

  bool _loaded = false;
  File? _file;

  Future<File> _stateFile() async {
    if (_file != null) return _file!;
    final dir = await getApplicationSupportDirectory();
    return _file = File('${dir.path}/app_mode.json');
  }

  Future<void> ensureLoaded() async {
    if (_loaded) return;
    try {
      final f = await _stateFile();
      if (await f.exists()) {
        final json = jsonDecode(await f.readAsString()) as Map<String, dynamic>;
        final raw = json['mode'] as String?;
        mode.value = switch (raw) {
          'personal' => AppModeKind.personal,
          'doctor' => AppModeKind.doctor,
          _ => null,
        };
      }
    } catch (_) {
      // 读坏了不致命:退回「未选择」,首屏会重新问一次。
    }
    _loaded = true;
  }

  Future<void> _save() async {
    try {
      final f = await _stateFile();
      await f.writeAsString(jsonEncode({'mode': mode.value?.name}));
    } catch (_) {}
  }

  /// 首次选择模式(「你是?」选择屏调)。
  Future<void> chooseMode(AppModeKind kind) async {
    await ensureLoaded();
    mode.value = kind;
    await _save();
  }

  /// 切换模式(设置页「切换模式」调),与 [chooseMode] 逻辑相同——分开命名只是
  /// 让调用点的意图更清楚(首次选择 vs 之后切换)。
  Future<void> setMode(AppModeKind kind) => chooseMode(kind);
}
