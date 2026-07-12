import 'package:flutter_test/flutter_test.dart';
import 'package:patrol/patrol.dart';
import 'package:mobile_flutter/main.dart' as app;
import 'package:mobile_flutter/src/rust/frb_generated.dart';

// MedMe Flutter 集成测试(Patrol)——健康档案 tab。
//   patrol test -t integration_test/archive_test.dart
//
// 测试环境是刚打开的空保险箱(没有导入过数据),所以这里验证的是空态引导:
// 切到「健康档案」tab 后能正常加载(不崩溃/不卡在转圈),患者头卡兜底文案
// 「我的健康档案」和空态引导文案「还没有病历」都可见。

void main() {
  patrolTest('健康档案 tab:切换后能加载,空库显示空态引导', ($) async {
    await RustLib.init();
    await $.pumpWidgetAndSettle(const app.MedMeApp());

    // 切到「健康档案」tab。
    await $('健康档案').tap();
    await $.pumpAndSettle();

    // 患者头卡:空库无姓名时兜底显示「我的健康档案」。
    expect($('我的健康档案'), findsOneWidget);

    // 空态引导可见,而不是白屏/一直转圈。
    expect($('还没有病历'), findsOneWidget);
  });
}
