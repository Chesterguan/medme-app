import 'package:flutter/material.dart';

import 'package:mobile_flutter/app_mode.dart';
import 'package:mobile_flutter/theme.dart';

/// 首次打开 App 的「你是?」选择屏——只在 [AppMode.instance.mode] 还没选过时
/// 显示(见 `main.dart` 的 `AppRoot`)。选完写入持久化,`AppRoot` 监听同一个
/// notifier 自动切进对应模式的主界面,本屏无需自己导航。
class ModePickerScreen extends StatelessWidget {
  const ModePickerScreen({super.key});

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: SafeArea(
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 24),
          child: Column(
            mainAxisAlignment: MainAxisAlignment.center,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              ClipRRect(
                borderRadius: BorderRadius.circular(18),
                child: Image.asset(
                  'assets/icon/app_icon.png',
                  width: 72,
                  height: 72,
                ),
              ),
              const SizedBox(height: 20),
              const Text(
                '你是?',
                textAlign: TextAlign.center,
                style: TextStyle(fontSize: 24, fontWeight: FontWeight.w800),
              ),
              const SizedBox(height: 8),
              const Text(
                '选一个开始使用;之后随时可以在「设置」里切换',
                textAlign: TextAlign.center,
                style: TextStyle(fontSize: 13.5, color: MedMe.faint),
              ),
              const SizedBox(height: 32),
              _ModeCard(
                icon: Icons.folder_shared_outlined,
                accentColor: MedMe.teal,
                accentSoft: MedMe.tealSoft,
                title: '我管自己/家人的病历',
                subtitle: '整理、查看、加密分享自己和家人的病历',
                onTap: () => AppMode.instance.chooseMode(AppModeKind.personal),
              ),
              const SizedBox(height: 16),
              _ModeCard(
                icon: Icons.medical_services_outlined,
                accentColor: MedMe.proxyOrange,
                accentSoft: MedMe.proxyOrangeSoft,
                title: '我是医生,帮病人建档',
                subtitle: '当面为病人拍摄纸质病历材料,生成加密文件交给病人',
                onTap: () => AppMode.instance.chooseMode(AppModeKind.doctor),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _ModeCard extends StatelessWidget {
  const _ModeCard({
    required this.icon,
    required this.accentColor,
    required this.accentSoft,
    required this.title,
    required this.subtitle,
    required this.onTap,
  });

  final IconData icon;
  final Color accentColor;
  final Color accentSoft;
  final String title;
  final String subtitle;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return Card(
      child: InkWell(
        borderRadius: BorderRadius.circular(14),
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.all(18),
          child: Row(
            children: [
              CircleAvatar(
                radius: 26,
                backgroundColor: accentSoft,
                child: Icon(icon, color: accentColor, size: 26),
              ),
              const SizedBox(width: 14),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      title,
                      style: const TextStyle(
                        fontSize: 16,
                        fontWeight: FontWeight.w700,
                      ),
                    ),
                    const SizedBox(height: 4),
                    Text(
                      subtitle,
                      style: const TextStyle(fontSize: 13, color: MedMe.faint, height: 1.4),
                    ),
                  ],
                ),
              ),
              const Icon(Icons.chevron_right, color: MedMe.faint),
            ],
          ),
        ),
      ),
    );
  }
}
