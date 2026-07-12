import 'package:flutter/material.dart';
import 'package:path_provider/path_provider.dart';
import 'package:mobile_flutter/src/rust/api/simple.dart';
import 'package:mobile_flutter/src/rust/frb_generated.dart';

// MedMe 品牌色(teal),与桌面/现有设计一致。
const _teal = Color(0xFF1789C1);

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
      theme: ThemeData(colorSchemeSeed: _teal, useMaterial3: true),
      debugShowCheckedModeBanner: false,
      home: const SmokeScreen(),
    );
  }
}

/// P1 冒烟屏:调 Rust 核 `vault_smoke`,在真实沙盒目录开保险箱并回显记录数,
/// 证明 Flutter → FRB → 现有 Rust 数据核 → 回来 整条链路打通。
class SmokeScreen extends StatefulWidget {
  const SmokeScreen({super.key});
  @override
  State<SmokeScreen> createState() => _SmokeScreenState();
}

class _SmokeScreenState extends State<SmokeScreen> {
  String _status = '正在连通 Rust 数据核…';

  @override
  void initState() {
    super.initState();
    _runSmoke();
  }

  Future<void> _runSmoke() async {
    try {
      final docs = await getApplicationDocumentsDirectory();
      final result = await vaultSmoke(dir: '${docs.path}/medme_vault');
      setState(() => _status = result);
    } catch (e) {
      setState(() => _status = '失败:$e');
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        backgroundColor: _teal,
        foregroundColor: Colors.white,
        title: const Text('MedMe 医我 · Flutter P1'),
      ),
      body: Center(
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              const Icon(Icons.health_and_safety, size: 64, color: _teal),
              const SizedBox(height: 16),
              Text(_status,
                  textAlign: TextAlign.center,
                  style: const TextStyle(fontSize: 16)),
            ],
          ),
        ),
      ),
    );
  }
}
