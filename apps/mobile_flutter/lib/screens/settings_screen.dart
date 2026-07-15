import 'dart:io';

import 'package:flutter/material.dart';
import 'package:mobile_flutter/src/rust/api/dto.dart';
import 'package:mobile_flutter/src/rust/api/vault.dart';
import 'package:mobile_flutter/theme.dart';
import 'package:mobile_flutter/vault_events.dart';
import 'package:mobile_flutter/vault_boot.dart';
import 'package:mobile_flutter/profile_manager.dart';
import 'package:mobile_flutter/icloud_bridge.dart';
import 'package:url_launcher/url_launcher.dart';

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
  // iCloud 容器是否可用(登录了 iCloud):Rust 拿不到,由原生 channel 判断。
  bool _icloudAvailable = false;

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
      final available = await IcloudBridge.available(); // 原生判断容器是否可用
      if (!mounted) return;
      setState(() {
        _icloud = results[0] as IcloudStatusDto;
        _profile = results[1] as PatientProfileDto;
        _icloudAvailable = available;
      });
    } catch (_) {
      // 状态读取失败不影响本屏其它功能(载入示例/清空仍可用),静默忽略即可。
    }
  }

  void _showSnack(String text) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(text)));
  }

  Future<void> _openHomepage() async {
    final uri = Uri.parse('https://chesterguan.github.io/medme/');
    final ok = await launchUrl(uri, mode: LaunchMode.externalApplication);
    if (!ok) _showSnack('无法打开主页,请稍后重试');
  }

  Future<void> _loadDemoData() async {
    setState(() => _busy = true);
    try {
      final n = await loadDemoData();
      bumpVaultRevision(); // 通知「健康档案」屏自动重载(并按识别姓名自动命名档案)
      await _refresh();
      goToArchive(); // 载入完直接跳到「健康档案」,不用用户再手点
      _showSnack('已载入 $n 份示例病历');
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
        title: const Text('清空所有数据?'),
        content: const Text(
          '确定清空全部记录?所有成员的示例数据和已导入病历都会被删除,'
          '保险箱恢复到初始状态,此操作不可撤销。',
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
      await wipeAllData(); // 全清:所有成员 vault + 份数缓存 + 待确认 + 恢复出厂
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
          _SectionLabel('保险箱'),
          _VaultCard(profile: _profile, onChanged: () => setState(() {})),
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
                // 灰字说明与点击后的确认弹窗内容重复,去掉省空间(用户反馈)。
                danger: true,
                onTap: _busy ? null : _confirmAndResetVault,
              ),
            ],
          ),
          // iCloud 同步是 iOS 原生能力(苹果设备间同步),安卓无 iCloud——不显示这一节,
          // 否则安卓用户会看到一个永远开不了、还叫他「去系统设置登录 iCloud」的死开关。
          // 与 OCR 一样,属于「各平台用自己的原生方案」的有意差异(安卓云同步见路线图 1.3)。
          if (Platform.isIOS) ...[
            _SectionLabel('iCloud 同步(实验性)'),
            _SettingsGroup(
              children: [
                _SettingsRow(
                  icon: (_icloud?.enabled ?? false)
                      ? Icons.cloud_done_outlined
                      : Icons.cloud_outlined,
                  title: 'iCloud 同步',
                  subtitle: _icloudSubtitle(),
                  trailing: Switch(
                    value: _icloud?.enabled ?? false,
                    onChanged: (_busy || !_icloudAvailable)
                        ? null
                        : _toggleIcloud,
                  ),
                ),
              ],
            ),
          ],
          _SectionLabel('关于'),
          _SettingsGroup(
            children: [
              _SettingsRow(
                icon: Icons.home_outlined,
                title: 'MedMe 主页',
                subtitle: '了解更多、下载其它平台版本',
                onTap: _openHomepage,
              ),
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

  String _icloudSubtitle() {
    if (_icloud == null) return '正在查询…';
    if (!_icloudAvailable) {
      return '请先在系统「设置」登录 iCloud 并开启 iCloud 云盘,再回来开启同步';
    }
    if (!_icloud!.enabled) return '开启后病历会同步到你其它苹果设备(实验性,建议先备份)';
    return '已开启 · 可在「文件」App → iCloud 云盘 → MedMe 医我 里看到已同步的病历';
  }

  Future<void> _toggleIcloud(bool want) async {
    if (want) {
      final ok = await showDialog<bool>(
        context: context,
        builder: (context) => AlertDialog(
          title: const Text('开启 iCloud 同步?'),
          content: const Text(
            '会把你的病历(真相数据)搬进本 App 的 iCloud 空间,在你登录同一 Apple ID 的'
            '苹果设备间自动同步;数据库仍留在本机。\n\n这是实验性功能,建议先用「导出」备份一份。',
            style: TextStyle(fontSize: 13.5, height: 1.5),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.of(context).pop(false),
              child: const Text('取消'),
            ),
            FilledButton(
              onPressed: () => Navigator.of(context).pop(true),
              child: const Text('开启'),
            ),
          ],
        ),
      );
      if (ok != true) return;
    }

    setState(() => _busy = true);
    try {
      if (want) {
        final container = await IcloudBridge.containerPath();
        if (container == null) {
          throw 'iCloud 当前不可用,请确认已登录 iCloud 并开启 iCloud 云盘';
        }
        await enableIcloudSync(containerDir: container);
      } else {
        await disableIcloudSync();
      }
      bumpVaultRevision(); // 保险箱已重开,通知档案屏刷新
      await _refresh();
      _showSnack(want ? '已开启 iCloud 同步' : '已关闭(本机保留一份副本)');
    } catch (e) {
      _showSnack('操作失败:$e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }
}

/// 保险箱卡:展示 + 可改**保险箱名字**(整个箱子的名字,不是某个人),多成员时列出
/// 每位成员各有多少份档案。切换成员/导入/加成员都在「健康档案」页,这里不重复那些
/// 功能——只做「这是哪个箱子、里面各人多少份」。名字默认「我的医疗档案」,不用「我」。
class _VaultCard extends StatelessWidget {
  const _VaultCard({required this.profile, required this.onChanged});
  final PatientProfileDto? profile;
  final VoidCallback onChanged;

  /// 当前成员用刚查到的最新记录数,其余成员用缓存(没加载过为 null)。
  int? _countOf(String member) {
    final pm = ProfileManager.instance;
    if (member == pm.current && profile != null) return profile!.recordCount;
    return pm.countFor(member);
  }

  Future<void> _rename(BuildContext context) async {
    final pm = ProfileManager.instance;
    final controller = TextEditingController(text: pm.vaultName);
    final name = await showDialog<String>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('保险箱名字'),
        content: TextField(
          controller: controller,
          autofocus: true,
          decoration: const InputDecoration(hintText: '例如:我家、张建国的病历'),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(controller.text),
            child: const Text('保存'),
          ),
        ],
      ),
    );
    if (name != null && name.trim().isNotEmpty) {
      await pm.setVaultName(name);
      onChanged();
    }
  }

  @override
  Widget build(BuildContext context) {
    final pm = ProfileManager.instance;
    final members = pm.members;
    final multi = members.length > 1;
    return Card(
      child: Column(
        children: [
          ListTile(
            leading: const CircleAvatar(
              radius: 24,
              backgroundColor: MedMe.tealSoft,
              child: Icon(Icons.folder_shared, color: MedMe.teal, size: 26),
            ),
            title: Text(
              pm.vaultName,
              style: const TextStyle(fontSize: 16, fontWeight: FontWeight.w700),
            ),
            subtitle: Text(
              multi
                  ? '${members.length} 位成员'
                  : '${_countOf(pm.current) ?? 0} 份记录',
              style: const TextStyle(color: MedMe.faint),
            ),
            trailing: IconButton(
              icon: const Icon(Icons.edit_outlined, color: MedMe.faint),
              tooltip: '改名字',
              onPressed: () => _rename(context),
            ),
          ),
          if (multi) ...[
            const Divider(height: 1, color: MedMe.line),
            for (final m in members)
              Padding(
                padding: const EdgeInsets.fromLTRB(20, 8, 20, 8),
                child: Row(
                  children: [
                    Expanded(
                      child: Text(m, style: const TextStyle(fontSize: 14)),
                    ),
                    Text(
                      _countOf(m) == null ? '—' : '${_countOf(m)} 份',
                      style: const TextStyle(color: MedMe.faint, fontSize: 13),
                    ),
                  ],
                ),
              ),
            const SizedBox(height: 6),
          ],
        ],
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
    this.subtitle,
    this.onTap,
    this.trailing,
    this.danger = false,
  });

  final IconData icon;
  final String title;
  final String? subtitle;
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
      subtitle: subtitle == null
          ? null
          : Text(subtitle!, style: const TextStyle(color: MedMe.faint)),
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
