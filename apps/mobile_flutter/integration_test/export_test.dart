import 'package:flutter_test/flutter_test.dart';
import 'package:patrol/patrol.dart';
import 'package:mobile_flutter/main.dart' as app;
import 'package:mobile_flutter/src/rust/frb_generated.dart';

// 新导航集成测试:导入入口移到「健康档案」右上角;「导出分享」独立成 tab。
// 验证:① 健康档案右上角「导入」入口在 ② 导出分享 tab 两张动作卡在,且导出/
// 加密分享点开的是应用内确认对话框(不拉系统),验证文案后取消关闭。
// 不真正触发采集(会拉起系统相机/相册/文件选择器),不依赖真实保险箱数据。

void main() {
  patrolTest('新导航:健康档案有导入入口,导出分享可开确认弹窗', ($) async {
    await RustLib.init();
    await $.pumpWidgetAndSettle(const app.MedMeApp());

    // 默认「健康档案」,右上角「导入」入口在。
    expect($('导入'), findsWidgets);

    // 切到「导出分享」,两张动作卡在。
    await $('导出分享').tap();
    expect($('导出时间线'), findsWidgets);
    expect($('加密分享给医生'), findsWidgets);

    // 导出:点按钮开应用内对话框,确认文案在,取消关闭。
    await $('选择范围并导出').tap();
    expect($('导出并分享'), findsOneWidget);
    await $('取消').tap();

    // 加密分享:点按钮开有效期对话框,取消关闭。
    await $('生成加密分享').tap();
    expect($('生成分享'), findsOneWidget);
    await $('取消').tap();
  });
}
