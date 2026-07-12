import 'package:flutter/material.dart';
import 'package:mobile_flutter/src/rust/frb_generated.dart';
import 'package:mobile_flutter/theme.dart';
import 'package:mobile_flutter/screens/import_export_screen.dart';
import 'package:mobile_flutter/screens/archive_screen.dart';
import 'package:mobile_flutter/screens/settings_screen.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await RustLib.init();
  // 保险箱在 P2 的 open_vault 落地后于此初始化(在真实沙盒/iCloud 目录打开);
  // 骨架阶段先不调,避免依赖尚未合并的 FFI。
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
      home: const HomeShell(),
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
