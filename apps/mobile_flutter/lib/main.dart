import 'package:flutter/material.dart';
import 'package:path_provider/path_provider.dart';
import 'package:mobile_flutter/src/rust/frb_generated.dart';
import 'package:mobile_flutter/src/rust/api/vault.dart';
import 'package:mobile_flutter/theme.dart';
import 'package:mobile_flutter/screens/import_export_screen.dart';
import 'package:mobile_flutter/screens/archive_screen.dart';
import 'package:mobile_flutter/screens/settings_screen.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await RustLib.init();
  runApp(const MedMeApp());
}

class MedMeApp extends StatelessWidget {
  const MedMeApp({super.key});
  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'MedMe 医我',
      theme: MedMe.theme(),
      debugShowCheckedModeBanner: false,
      home: const VaultBootstrap(),
    );
  }
}

/// 启动引导:先在真实沙盒目录打开保险箱(FFI `open_vault`),再进主界面。
/// 打开是可韧性的(损坏的派生 db 会从 log 重建);目录取自 path_provider。
/// iCloud 暂关(P5 接);打开失败给人性化提示而非白屏。
class VaultBootstrap extends StatefulWidget {
  const VaultBootstrap({super.key});
  @override
  State<VaultBootstrap> createState() => _VaultBootstrapState();
}

class _VaultBootstrapState extends State<VaultBootstrap> {
  late final Future<void> _open = _openVault();

  Future<void> _openVault() async {
    final docs = await getApplicationDocumentsDirectory();
    final support = await getApplicationSupportDirectory();
    await openVault(
      docsDir: docs.path,
      dataDir: support.path,
      icloudEnabled: false, // P5 接 iCloud
    );
  }

  @override
  Widget build(BuildContext context) {
    return FutureBuilder<void>(
      future: _open,
      builder: (context, snap) {
        if (snap.connectionState != ConnectionState.done) {
          return const Scaffold(
            body: Center(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Icon(Icons.health_and_safety, size: 56, color: MedMe.teal),
                  SizedBox(height: 16),
                  CircularProgressIndicator(color: MedMe.teal),
                ],
              ),
            ),
          );
        }
        if (snap.hasError) {
          return Scaffold(
            body: Center(
              child: Padding(
                padding: const EdgeInsets.all(24),
                child: Text(
                  '无法打开你的健康档案:\n${snap.error}\n\n请重启 App 再试。',
                  textAlign: TextAlign.center,
                  style: const TextStyle(fontSize: 15),
                ),
              ),
            ),
          );
        }
        return const HomeShell();
      },
    );
  }
}

/// 底部导航壳:三个一级 tab —— 导入导出 / 健康档案 / 设置。
/// 「导入导出」按用户要求提升为一级 tab(不埋设置里),后续导出维度会变多。
class HomeShell extends StatefulWidget {
  const HomeShell({super.key});
  @override
  State<HomeShell> createState() => _HomeShellState();
}

class _HomeShellState extends State<HomeShell> {
  int _index = 0;

  static const _screens = [
    ImportExportScreen(),
    ArchiveScreen(),
    SettingsScreen(),
  ];

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: IndexedStack(index: _index, children: _screens),
      bottomNavigationBar: NavigationBar(
        selectedIndex: _index,
        onDestinationSelected: (i) => setState(() => _index = i),
        destinations: const [
          NavigationDestination(
            icon: Icon(Icons.swap_vert_outlined),
            selectedIcon: Icon(Icons.swap_vert),
            label: '导入导出',
          ),
          NavigationDestination(
            icon: Icon(Icons.folder_outlined),
            selectedIcon: Icon(Icons.folder),
            label: '健康档案',
          ),
          NavigationDestination(
            icon: Icon(Icons.settings_outlined),
            selectedIcon: Icon(Icons.settings),
            label: '设置',
          ),
        ],
      ),
    );
  }
}
