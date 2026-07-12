import 'package:flutter_test/flutter_test.dart';
import 'package:patrol/patrol.dart';
import 'package:mobile_flutter/main.dart' as app;
import 'package:mobile_flutter/src/rust/frb_generated.dart';

// MedMe Flutter 集成测试(Patrol)。用户跑:
//   dart pub global activate patrol_cli
//   patrol test -t integration_test/app_test.dart
// 我负责写用例/流程,用户负责跑并反馈结果。
//
// P1/P3 骨架冒烟:app 能启动(RustLib 初始化不崩)、底部三个一级 tab
// (导入导出/健康档案/设置)都在且可点击切换。随阶段推进这里会加端到端流程:
// 载入示例数据→档案有记录、导入 PDF/图片、清空重置、导出、加密分享等。

void main() {
  patrolTest('骨架冒烟:启动 + 底部三个一级 tab 存在且可切换', ($) async {
    await RustLib.init();
    await $.pumpWidgetAndSettle(const app.MedMeApp());

    // 底部三个一级 tab 标签都在 → 骨架渲染成功。
    expect($('导入导出'), findsOneWidget);
    expect($('健康档案'), findsWidgets);
    expect($('设置'), findsWidgets);

    // 依次点击切换,不崩溃。
    await $('健康档案').tap();
    await $('设置').tap();
    await $('导入导出').tap();
  });
}
