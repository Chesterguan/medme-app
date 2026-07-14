import 'dart:io';

import 'package:path_provider/path_provider.dart';

import 'package:mobile_flutter/src/rust/api/vault.dart';
import 'package:mobile_flutter/icloud_bridge.dart';
import 'package:mobile_flutter/profile_manager.dart';
import 'package:mobile_flutter/review_state.dart';
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

/// 「清空所有数据」= 恢复出厂:清**所有成员、所有位置**的 vault 数据(本机 + iCloud
/// 容器)+ 份数缓存 + 待确认,重置成单一默认档案。
///
/// ⚠️ root 成员有**两处** vault:本机 `<docs>/vault` 与 iCloud `<container>/Documents/vault`
/// (关 iCloud 时容器副本会被 `disable_icloud_sync` 保留)。`resetVault` 只干净清掉**当前
/// 活跃**那处(正常关连接 + 删 db/wal + 重开空);另一处必须显式删,否则清空后容器里仍留
/// 整份病历、再开 iCloud 会 adopt 回来(评审 Critical)。子成员整个 `profiles/` 删掉。
Future<void> wipeAllData() async {
  final docsRoot = (await getApplicationDocumentsDirectory()).path;
  final containerRoot = await IcloudBridge.containerPath();

  Future<void> rmDir(String path) async {
    final d = Directory(path);
    if (await d.exists()) await d.delete(recursive: true);
  }

  // 1. 注册表恢复出厂(current→默认 root)+ 清待确认。
  await ProfileManager.instance.factoryReset();
  await ReviewState.instance.clearAll();

  // 2. 重开默认(root)vault → 活跃 = root;3. resetVault 干净清活跃那处(含 db/wal)+ 重开空。
  await openCurrentProfileVault();
  await resetVault();

  // 4. 删 root 的**非活跃**那处 vault 副本(resetVault 没碰到的):iCloud 开时活跃=容器,
  //    非活跃=本机;关时反之。icloudStatus().enabled 与 Rust 的路径决策同源(同一 marker)。
  final icloudOn = (await icloudStatus()).enabled;
  if (icloudOn) {
    await rmDir('$docsRoot/vault');
  } else if (containerRoot != null) {
    await rmDir('$containerRoot/Documents/vault');
  }

  // 5. 删所有子成员数据(各自 vault + 派生库都在 profiles/ 内);本机 + iCloud 容器都删。
  for (final root in [docsRoot, if (containerRoot != null) '$containerRoot/Documents']) {
    await rmDir('$root/profiles');
  }

  bumpVaultRevision();
}

/// 用报告里识别到的患者姓名,给还没定过名的默认档案自动命名(迁移其待确认/标红键)。
/// 导入、载入示例、档案加载等任一有患者姓名的地方都可调,幂等:只在首次未命名时生效。
Future<void> autoNameCurrentProfileFrom(String? detectedName) async {
  if (detectedName == null || detectedName.trim().isEmpty) return;
  final old = ProfileManager.instance.current;
  final renamed = await ProfileManager.instance.maybeAutoNameRoot(detectedName);
  if (renamed != null) await ReviewState.instance.renameMember(old, renamed);
}

/// 新建成员(空库)并切过去、重开、刷新。
Future<void> createProfileAndReopen(String name) async {
  await ProfileManager.instance.create(name);
  await openCurrentProfileVault();
  bumpVaultRevision();
}
