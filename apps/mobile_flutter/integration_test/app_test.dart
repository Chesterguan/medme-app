import 'package:flutter_test/flutter_test.dart';
import 'package:patrol/patrol.dart';
import 'package:mobile_flutter/main.dart' as app;
import 'package:mobile_flutter/src/rust/frb_generated.dart';

// MedMe Flutter 集成测试(Patrol)。用户跑:
//   dart pub global activate patrol_cli
//   patrol test -t integration_test/app_test.dart
// 我写用例/流程,用户跑并反馈。
//
// 骨架冒烟:app 能启动(RustLib 初始化不崩)、底部三个一级 tab
// (健康档案 / 导出分享 / 设置)都在且可点击切换。默认停在「健康档案」。

void main() {
  patrolTest('骨架冒烟:启动 + 底部三个一级 tab 存在且可切换', ($) async {
    await RustLib.init();
    await $.pumpWidgetAndSettle(const app.MedMeApp());

    expect($('健康档案'), findsWidgets);
    expect($('导出分享'), findsWidgets);
    expect($('设置'), findsWidgets);

    await $('导出分享').tap();
    await $('设置').tap();
    await $('健康档案').tap();
  });
}
