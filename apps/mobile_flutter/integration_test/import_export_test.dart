import 'package:flutter_test/flutter_test.dart';
import 'package:patrol/patrol.dart';
import 'package:mobile_flutter/main.dart' as app;
import 'package:mobile_flutter/src/rust/frb_generated.dart';

// 「导入导出」屏(底部第一个一级 tab)集成测试:验证屏能加载、三个采集入口
// (拍照 / 从相册选 / 选择文件)与两个导出/分享入口(导出时间线 / 加密分享给
// 医生)都在且可点。采集三项会拉起系统相机 / 相册 / 文件选择器,不在此测试里
// 真正触发(留给后续端到端流程,见 `app_test.dart` 顶部注释);导出/分享两项
// 点开的是应用内确认对话框,验证弹窗内容后点「取消」关闭,不依赖真实保险箱
// 数据、不需要真机权限。

void main() {
  patrolTest('导入导出屏:入口可见可点,导出/分享弹窗能开能关', ($) async {
    await RustLib.init();
    await $.pumpWidgetAndSettle(const app.MedMeApp());

    // 默认停在第一个 tab(导入导出),标题在。
    expect($('导入 · 导出'), findsOneWidget);

    // 三个采集入口都在(不点——会拉起系统相机/相册/文件选择器)。
    expect($('拍照'), findsOneWidget);
    expect($('从相册选'), findsOneWidget);
    expect($('选择文件'), findsOneWidget);

    // 导出时间线:点开应用内确认对话框,确认文案在,再取消关闭。
    expect($('导出时间线(HTML)'), findsOneWidget);
    await $('导出时间线(HTML)').tap();
    expect($('导出并分享'), findsOneWidget);
    await $('取消').tap();

    // 加密分享给医生:点开有效期选择对话框,确认文案在,再取消关闭。
    expect($('加密分享给医生'), findsOneWidget);
    await $('加密分享给医生').tap();
    expect($('生成分享'), findsOneWidget);
    await $('取消').tap();
  });
}
