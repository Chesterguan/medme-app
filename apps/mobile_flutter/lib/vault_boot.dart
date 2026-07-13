import 'package:path_provider/path_provider.dart';

import 'package:mobile_flutter/src/rust/api/vault.dart';
import 'package:mobile_flutter/icloud_bridge.dart';
import 'package:mobile_flutter/profile_manager.dart';
import 'package:mobile_flutter/vault_events.dart';

/// 打开「当前成员」的保险箱:按 [ProfileManager] 组合本机/iCloud 路径,调 Rust
/// `open_vault`(Rust 的进程级 vault 会被替换成该成员的)。启动 + 切换成员后都调它。
///
/// data 目录(设备 id、iCloud 全局开关标记、导入临时文件)所有成员共用——iCloud 是
/// 全局开关(开了对所有成员生效);派生库则每成员独立(见 Rust `resolve_vault_paths`)。
Future<void> openCurrentProfileVault() async {
  await ProfileManager.instance.ensureLoaded();
  final docsRoot = (await getApplicationDocumentsDirectory()).path;
  final support = (await getApplicationSupportDirectory()).path;
  final containerRoot = await IcloudBridge.containerPath();

  await openVault(
    docsDir: ProfileManager.instance.localBase(docsRoot),
    dataDir: support,
    icloudContainerDir: ProfileManager.instance.containerBase(containerRoot),
  );
}

/// 切换到某成员并重开其保险箱,然后通知各屏刷新。
Future<void> switchProfileAndReopen(String name) async {
  await ProfileManager.instance.switchTo(name);
  await openCurrentProfileVault();
  bumpVaultRevision();
}

/// 新建成员(空库)并切过去、重开、刷新。
Future<void> createProfileAndReopen(String name) async {
  await ProfileManager.instance.create(name);
  await openCurrentProfileVault();
  bumpVaultRevision();
}
