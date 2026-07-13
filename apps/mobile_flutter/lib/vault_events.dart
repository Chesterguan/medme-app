import 'package:flutter/foundation.dart';

/// 保险箱内容变更的全局信号。导入、清空、载入示例后调用 [bumpVaultRevision]，
/// 监听者(尤其「健康档案」屏)据此重新加载。
///
/// 为什么需要:底部三 tab 用 `IndexedStack` 承载,切走的屏会**保活**(state 不销毁),
/// 所以在「设置」里清空、或在「导入导出」里导入后,「健康档案」屏的 `initState` 不会
/// 再跑一次 → 切回去还是旧数据,用户以为没生效。让档案屏监听这个信号即可自动刷新。
final ValueNotifier<int> vaultRevision = ValueNotifier<int>(0);

/// 保险箱内容变了(导入/清空/载入示例),通知所有监听屏重载。
void bumpVaultRevision() => vaultRevision.value++;

/// 当前底部一级 tab 下标(0=健康档案,1=导出分享,2=设置)。`HomeShell` 监听它
/// 切换页面 —— 让「设置」里载入示例后能自动跳回「健康档案」,不用用户再手点。
final ValueNotifier<int> selectedTab = ValueNotifier<int>(0);

/// 跳到「健康档案」tab。
void goToArchive() => selectedTab.value = 0;
