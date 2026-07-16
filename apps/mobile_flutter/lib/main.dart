import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:mobile_flutter/src/rust/frb_generated.dart';
import 'package:mobile_flutter/theme.dart';
import 'package:mobile_flutter/screens/archive_screen.dart';
import 'package:mobile_flutter/screens/export_screen.dart';
import 'package:mobile_flutter/screens/settings_screen.dart';
import 'package:mobile_flutter/vault_boot.dart';
import 'package:mobile_flutter/vault_events.dart';

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
      // 面向简体中文用户:强制中文本地化,日历选择器/所有 Material 弹窗都显示中文。
      locale: const Locale('zh', 'CN'),
      supportedLocales: const [Locale('zh', 'CN'), Locale('en')],
      localizationsDelegates: const [
        GlobalMaterialLocalizations.delegate,
        GlobalWidgetsLocalizations.delegate,
        GlobalCupertinoLocalizations.delegate,
      ],
      home: const VaultBootstrap(),
    );
  }
}

/// 启动引导:先在真实沙盒目录打开保险箱(FFI `open_vault`),再进主界面。
/// 打开是可韧性的(损坏的派生 db 会从 log 重建);目录取自 path_provider。
/// iCloud 已接入(见 `vault_boot` / `icloud_bridge`):容器可解析且用户在设置里开启
/// 同步时,真相存进 iCloud 容器,否则用本机沙盒。打开失败给人性化提示而非白屏。
class VaultBootstrap extends StatefulWidget {
  const VaultBootstrap({super.key});
  @override
  State<VaultBootstrap> createState() => _VaultBootstrapState();
}

class _VaultBootstrapState extends State<VaultBootstrap> {
  // 打开「当前成员」的保险箱(多成员见 profile_manager / vault_boot)。
  final Future<void> _open = openCurrentProfileVault();

  @override
  Widget build(BuildContext context) {
    return FutureBuilder<void>(
      future: _open,
      builder: (context, snap) {
        if (snap.connectionState != ConnectionState.done) {
          return Scaffold(
            body: Center(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  ClipRRect(
                    borderRadius: BorderRadius.circular(18),
                    child: Image.asset(
                      'assets/icon/app_icon.png',
                      width: 84,
                      height: 84,
                    ),
                  ),
                  const SizedBox(height: 20),
                  const CircularProgressIndicator(color: MedMe.teal),
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

/// 底部导航壳:三个一级 tab —— 健康档案 / 导出分享 / 设置。
/// 导入入口在「健康档案」页右上角「导入」按钮(不是独立 tab);导出/分享独立成一级 tab。
class HomeShell extends StatefulWidget {
  const HomeShell({super.key});
  @override
  State<HomeShell> createState() => _HomeShellState();
}

class _HomeShellState extends State<HomeShell> {
  int _index = 0;

  // 健康档案(看 + 右上角导入)· 导出分享 · 设置。导入并进「健康档案」,
  // 导出/分享独立成 tab —— 手机端「轻」定位:采集 + 看 + 分享,搜索/趋势在桌面/查看器。
  static const _screens = [ArchiveScreen(), ExportScreen(), SettingsScreen()];

  @override
  void initState() {
    super.initState();
    // 别的屏(如设置载入示例后)可程序化切 tab。
    selectedTab.addListener(_onTabRequested);
  }

  @override
  void dispose() {
    selectedTab.removeListener(_onTabRequested);
    super.dispose();
  }

  void _onTabRequested() {
    if (mounted && selectedTab.value != _index) {
      setState(() => _index = selectedTab.value);
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: IndexedStack(index: _index, children: _screens),
      bottomNavigationBar: NavigationBar(
        selectedIndex: _index,
        // 统一走 selectedTab:手点和程序化跳转(设置载入示例后)同一条路径。
        onDestinationSelected: (i) => selectedTab.value = i,
        destinations: const [
          NavigationDestination(
            icon: Icon(Icons.folder_outlined),
            selectedIcon: Icon(Icons.folder),
            label: '健康档案',
          ),
          NavigationDestination(
            icon: Icon(Icons.ios_share_outlined),
            selectedIcon: Icon(Icons.ios_share),
            label: '导出分享',
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
