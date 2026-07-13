import 'package:flutter/material.dart';
import 'package:mobile_flutter/src/rust/api/dto.dart';
import 'package:mobile_flutter/src/rust/api/vault.dart';
import 'package:mobile_flutter/theme.dart';
import 'package:mobile_flutter/vault_events.dart';

/// 与 `pubspec.yaml` 的 `version:` 字段保持一致。P3 范围内没有为读版本号新增
/// `package_info_plus` 依赖(约束里明确不加新依赖),手工同步即可——这颗
/// 版本号本来就只在“关于”里给人看,不参与任何业务逻辑。
const _appVersion = '1.2.0';

/// 底部导航一级 tab「设置」—— 载入示例数据 / 清空重置 / iCloud 同步占位 / 关于。
/// 分组卡片列表,视觉还原自 `apps/mobile/src/App.tsx` 的设置区(sect + group + row)。
/// 保险箱在 `main.dart` 启动时已打开,这里直接调 FFI,不重复任何 Rust 侧逻辑。
class SettingsScreen extends StatefulWidget {
  const SettingsScreen({super.key});

  @override
  State<SettingsScreen> createState() => _SettingsScreenState();
}

class _SettingsScreenState extends State<SettingsScreen> {
  IcloudStatusDto? _icloud;
  PatientProfileDto? _profile;

  /// 载入示例 / 清空时置真,禁用所有操作按钮,防止重复点击(尤其清空——
  /// 用户反馈过「载入示例后清空点了没反应」,这里确保按钮忙时不可再点,
  /// 而不是悄悄丢弃点击)。
  bool _busy = false;

  @override
  void initState() {
    super.initState();
    _refresh();
    // 导入/清空等在别的 tab 发生时,身份卡的记录数等也要跟着更新(本屏保活)。
    vaultRevision.addListener(_refresh);
  }

  @override
  void dispose() {
    vaultRevision.removeListener(_refresh);
    super.dispose();
  }

  Future<void> _refresh() async {
    try {
      final results = await Future.wait([icloudStatus(), patientProfile()]);
      if (!mounted) return;
      setState(() {
        _icloud = results[0] as IcloudStatusDto;
        _profile = results[1] as PatientProfileDto;
      });
    } catch (_) {
      // 状态读取失败不影响本屏其它功能(载入示例/清空仍可用),静默忽略即可。
    }
  }

  void _showSnack(String text) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(text)));
  }

  Future<void> _loadDemoData() async {
    setState(() => _busy = true);
    try {
      final n = await loadDemoData();
      bumpVaultRevision(); // 通知「健康档案」屏自动重载
      await _refresh();
      _showSnack('已载入 $n 份示例病历,去「健康档案」查看');
    } catch (e) {
      _showSnack('载入示例数据失败:$e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _confirmAndResetVault() async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('清空保险箱?'),
        content: const Text(
          '确定清空全部记录?示例数据和已导入的病历都会被删除,'
          '此操作不可撤销。',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('取消'),
          ),
          TextButton(
            onPressed: () => Navigator.of(context).pop(true),
            style: TextButton.styleFrom(foregroundColor: MedMe.danger),
            child: const Text('清空'),
          ),
        ],
      ),
    );
    if (confirmed != true) return;

    setState(() => _busy = true);
    try {
      await resetVault();
      bumpVaultRevision(); // 通知「健康档案」屏自动清空重载
      await _refresh();
      _showSnack('已清空');
    } catch (e) {
      _showSnack('清空失败:$e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('设置')),
      body: ListView(
        padding: const EdgeInsets.fromLTRB(16, 8, 16, 32),
        children: [
          _SectionLabel('当前健康档案'),
          _ProfileHeader(profile: _profile),
          _SectionLabel('示例数据'),
          _SettingsGroup(
            children: [
              _SettingsRow(
                icon: Icons.download_outlined,
                title: '载入示例数据(张建国)',
                subtitle: '一键导入一份完整的示例病历,方便你先看看效果、再决定怎么用',
                onTap: _busy ? null : _loadDemoData,
              ),
            ],
          ),
          _SectionLabel('数据管理'),
          _SettingsGroup(
            children: [
              _SettingsRow(
                icon: Icons.delete_outline,
                title: '清空所有数据 · 重置保险箱',
                subtitle: '删除全部记录,回到初始空状态',
                danger: true,
                onTap: _busy ? null : _confirmAndResetVault,
              ),
            ],
          ),
          _SectionLabel('iCloud 同步'),
          _SettingsGroup(
            children: [
              _SettingsRow(
                icon: Icons.cloud_off_outlined,
                title: 'iCloud 同步',
                subtitle: _icloudSubtitle(_icloud),
                trailing: Switch(value: false, onChanged: null),
              ),
            ],
          ),
          _SectionLabel('关于'),
          _SettingsGroup(
            children: [
              _InfoRow(
                title: 'MedMe 医我',
                subtitle: 'v$_appVersion · 本地优先:你的病历只保存在你自己的设备上',
              ),
              const _InfoRow(
                title: '医疗免责声明',
                subtitle:
                    'MedMe 是个人病历整理工具,不是医疗器械,不提供诊断或治疗建议;'
                    '一切以原始医疗文件为准,请遵医嘱。',
              ),
            ],
          ),
        ],
      ),
    );
  }

  String _icloudSubtitle(IcloudStatusDto? status) {
    if (status == null) return '正在查询…';
    if (!status.available) return '此设备暂不可用,后续版本会支持一键开启同步';
    if (!status.enabled) return '暂未开启,即将在后续版本支持';
    return '已开启,自动同步到你的苹果设备';
  }
}

/// 身份卡:顶部醒目展示「这是谁的健康档案」——姓名 + 性别·年龄 + 记录数。
/// 让用户清楚当前档案属于谁(为后续家庭多成员/共享方案铺路)。姓名等来自
/// `patientProfile()`(据已导入病历推断);空库时给友好占位。
class _ProfileHeader extends StatelessWidget {
  const _ProfileHeader({required this.profile});
  final PatientProfileDto? profile;

  @override
  Widget build(BuildContext context) {
    final p = profile;
    final rawName = p?.name;
    final gender = p?.gender;
    final age = p?.age;
    final hasName = rawName != null && rawName.isNotEmpty;
    final hasRecords = p != null && p.recordCount > 0;
    final name = hasName ? rawName : (hasRecords ? '未命名档案' : '还没有健康档案');
    final meta = <String>[
      if (gender != null && gender.isNotEmpty) gender,
      if (age != null && age.isNotEmpty) age,
      if (p != null) '${p.recordCount} 份记录',
    ].join(' · ');

    return Card(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Row(
          children: [
            CircleAvatar(
              radius: 26,
              backgroundColor: MedMe.tealSoft,
              child: const Icon(Icons.person, color: MedMe.teal, size: 30),
            ),
            const SizedBox(width: 14),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    name,
                    style: const TextStyle(
                      fontSize: 17,
                      fontWeight: FontWeight.w700,
                      color: MedMe.ink,
                    ),
                  ),
                  if (meta.isNotEmpty) ...[
                    const SizedBox(height: 4),
                    Text(
                      meta,
                      style: const TextStyle(fontSize: 13, color: MedMe.faint),
                    ),
                  ],
                  const SizedBox(height: 6),
                  const Text(
                    '当前设备上的这份档案 · 多成员切换即将支持',
                    style: TextStyle(fontSize: 11.5, color: MedMe.faint),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// 分组标题(灰色小字),对应旧版 `App.css` 里的 `.sect`。
class _SectionLabel extends StatelessWidget {
  const _SectionLabel(this.text);
  final String text;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(4, 16, 4, 8),
      child: Text(
        text,
        style: const TextStyle(
          fontSize: 13,
          fontWeight: FontWeight.w600,
          color: MedMe.faint,
        ),
      ),
    );
  }
}

/// 白色圆角卡片,内部若干行,行间用分隔线隔开——对应旧版 `.group`。
class _SettingsGroup extends StatelessWidget {
  const _SettingsGroup({required this.children});
  final List<Widget> children;

  @override
  Widget build(BuildContext context) {
    return Card(
      child: Column(
        children: [
          for (var i = 0; i < children.length; i++) ...[
            if (i > 0) const Divider(height: 1, color: MedMe.line),
            children[i],
          ],
        ],
      ),
    );
  }
}

/// 可点击的一行:图标 + 标题 + 说明 + 尾部箭头(或自定义 trailing)。
/// 对应旧版 `.row`;`danger` 对应 `.row.danger`(清空按钮用 `MedMe.danger`)。
class _SettingsRow extends StatelessWidget {
  const _SettingsRow({
    required this.icon,
    required this.title,
    required this.subtitle,
    this.onTap,
    this.trailing,
    this.danger = false,
  });

  final IconData icon;
  final String title;
  final String subtitle;
  final VoidCallback? onTap;
  final Widget? trailing;
  final bool danger;

  @override
  Widget build(BuildContext context) {
    final color = danger ? MedMe.danger : MedMe.ink;
    return ListTile(
      leading: Icon(icon, color: danger ? MedMe.danger : MedMe.teal),
      title: Text(
        title,
        style: TextStyle(fontWeight: FontWeight.w600, color: color),
      ),
      subtitle: Text(subtitle, style: const TextStyle(color: MedMe.faint)),
      trailing:
          trailing ??
          (onTap != null
              ? const Icon(Icons.chevron_right, color: MedMe.faint)
              : null),
      onTap: onTap,
      enabled: onTap != null || trailing != null,
    );
  }
}

/// 纯展示的一行(无点击),用于「关于」里的静态信息。
class _InfoRow extends StatelessWidget {
  const _InfoRow({required this.title, required this.subtitle});
  final String title;
  final String subtitle;

  @override
  Widget build(BuildContext context) {
    return ListTile(
      title: Text(title, style: const TextStyle(fontWeight: FontWeight.w600)),
      subtitle: Text(subtitle, style: const TextStyle(color: MedMe.faint)),
    );
  }
}
