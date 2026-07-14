import 'package:flutter/material.dart';

import 'package:mobile_flutter/src/rust/api/dto.dart';
import 'package:mobile_flutter/src/rust/api/vault.dart';
import 'package:mobile_flutter/theme.dart';
import 'package:mobile_flutter/screens/document_detail.dart';
import 'package:mobile_flutter/vault_events.dart';
import 'package:mobile_flutter/import_flow.dart';
import 'package:mobile_flutter/review_state.dart';
import 'package:mobile_flutter/profile_manager.dart';
import 'package:mobile_flutter/vault_boot.dart';

/// 底部导航一级 tab「健康档案」—— 生命时间线:就诊组 + 独立文档,按日期倒序,
/// 点开看详情。与旧 Tauri 移动端 App.tsx 的 archive tab(phead + tl)同一观感,
/// 数据来自 FFI `loadArchive` / `patientProfile`(见 lib/src/rust/api/vault.dart)。

// doc_type / encounter kind → 中文标签(与 core-model types.rs、旧 App.tsx 一致)。
const Map<String, String> _docLabel = {
  'lab_report': '化验',
  'imaging_report': '影像',
  'discharge_summary': '出院小结',
  'prescription': '处方',
  'clinical_note': '病历',
  'pathology': '病理',
  'surgery': '手术',
  'other': '其他',
  'unknown': '待归类',
};
const Map<String, String> _kindLabel = {
  'inpatient': '住院',
  'outpatient': '门诊',
  'emergency': '急诊',
  'exam': '检查',
};

// 文档类型/就诊类型 → 图标 + 配色(与旧 App.css 的 t-* 徽标一致,只是用 Material
// 图标替代内联 SVG)。
const Map<String, IconData> _docIcon = {
  'lab_report': Icons.science_outlined,
  'imaging_report': Icons.document_scanner_outlined,
  'prescription': Icons.medication_outlined,
  'discharge_summary': Icons.bed_outlined,
  'clinical_note': Icons.medical_services_outlined,
  'pathology': Icons.biotech_outlined,
  'surgery': Icons.content_cut,
  'other': Icons.description_outlined,
  'unknown': Icons.help_outline,
};
const Map<String, IconData> _kindIcon = {
  'outpatient': Icons.medical_services_outlined,
  'inpatient': Icons.bed_outlined,
};
const Map<String, Color> _typeColor = {
  'lab_report': Color(0xFF1D4ED8),
  'imaging_report': Color(0xFFB45309),
  'prescription': Color(0xFF047857),
  'discharge_summary': Color(0xFF4338CA),
  'clinical_note': Color(0xFF0369A1),
  'pathology': Color(0xFFBE123C),
  'surgery': Color(0xFF7E22CE),
  'other': Color(0xFF475569),
  'unknown': Color(0xFF475569),
  'enc': Color(0xFF1D4ED8), // 就诊组统一用蓝(旧 App.css .t-enc)
};

Color _colorFor(String key) => _typeColor[key] ?? _typeColor['other']!;

IconData _iconForDoc(String docType) =>
    _docIcon[docType] ?? Icons.description_outlined;

IconData _iconForKind(String kind) =>
    _kindIcon[kind] ?? Icons.local_hospital_outlined;

String _fmtDate(String? iso) {
  if (iso == null || iso.isEmpty) return '';
  final d = DateTime.tryParse(iso);
  if (d == null) return '';
  return '${d.year}-${d.month.toString().padLeft(2, '0')}-${d.day.toString().padLeft(2, '0')}';
}

String _groupTitle(TimelineGroupDto g) {
  return switch (g) {
    TimelineGroupDto_Encounter(:final encounter) =>
      encounter.provider != null
          ? '${_kindLabel[encounter.kind] ?? encounter.kind} · ${encounter.provider}'
          : (_kindLabel[encounter.kind] ?? encounter.kind),
    TimelineGroupDto_Document(:final doc) =>
      doc.title ?? _docLabel[doc.docType] ?? '记录',
  };
}

String _groupDate(TimelineGroupDto g) {
  return switch (g) {
    TimelineGroupDto_Encounter(:final encounter) => _fmtDate(
      encounter.startDate,
    ),
    TimelineGroupDto_Document(:final doc) => _fmtDate(doc.docDate),
  };
}

String _groupDesc(TimelineGroupDto g) {
  return switch (g) {
    TimelineGroupDto_Encounter(:final encounter, :final docs) => () {
      final kinds = <String>{};
      for (final d in docs) {
        kinds.add(_docLabel[d.docType] ?? d.docType);
      }
      // 用实际 docs.length —— 待确认剔除后 `_confirmedOnly` 会重建只含已确认文档的组,
      // 此时 encounter.docCount(FFI 按全量算)会 stale,显示条数与展开数量对不上。
      final parts = ['${docs.length} 份记录', ...kinds.take(3)];
      if (encounter.transferred) parts.add('转院');
      return parts.join(' · ');
    }(),
    TimelineGroupDto_Document(:final doc) => [
      _docLabel[doc.docType] ?? doc.docType,
      if (doc.sliceCount != null) '影像 ${doc.sliceCount} 张',
    ].join(' · '),
  };
}

/// 把时间线分组拍平成文档列表(就诊组内文档 + 独立文档),用于「待确认」筛选。
List<DocumentSummaryDto> _allDocs(List<TimelineGroupDto> groups) {
  final out = <DocumentSummaryDto>[];
  for (final g in groups) {
    switch (g) {
      case TimelineGroupDto_Encounter(:final docs):
        out.addAll(docs);
      case TimelineGroupDto_Document(:final doc):
        out.add(doc);
    }
  }
  return out;
}

/// 「已确认」时间线:把待确认文档从分组里剔除(它们单独在顶部红框区展示,避免重复)。
/// 就诊组里若有部分文档待确认,重建一个只含已确认文档的组;整组都待确认则整组略去。
List<TimelineGroupDto> _confirmedOnly(List<TimelineGroupDto> groups) {
  final out = <TimelineGroupDto>[];
  for (final g in groups) {
    switch (g) {
      case TimelineGroupDto_Document(:final doc):
        if (!ReviewState.instance.isPending(doc.id)) out.add(g);
      case TimelineGroupDto_Encounter(:final encounter, :final docs):
        final kept = docs
            .where((d) => !ReviewState.instance.isPending(d.id))
            .toList();
        if (kept.isEmpty) continue;
        out.add(
          kept.length == docs.length
              ? g
              : TimelineGroupDto.encounter(encounter: encounter, docs: kept),
        );
    }
  }
  return out;
}

class ArchiveScreen extends StatefulWidget {
  const ArchiveScreen({super.key});

  @override
  State<ArchiveScreen> createState() => _ArchiveScreenState();
}

class _ArchiveScreenState extends State<ArchiveScreen> {
  late Future<(PatientProfileDto, List<TimelineGroupDto>)> _future = _load();
  // 已展开的就诊组(按 **encounter.id** 记,不用列表下标——删除/导入后下标会错位)。
  final Set<int> _expanded = {};

  @override
  void initState() {
    super.initState();
    // 导入/清空/载入示例后自动重载(本屏在 IndexedStack 里保活,initState 不会重跑)。
    vaultRevision.addListener(_onVaultChanged);
  }

  @override
  void dispose() {
    vaultRevision.removeListener(_onVaultChanged);
    super.dispose();
  }

  void _onVaultChanged() {
    if (mounted) _refresh();
  }

  Future<(PatientProfileDto, List<TimelineGroupDto>)> _load() async {
    final results = await Future.wait([patientProfile(), loadArchive()]);
    final profile = results[0] as PatientProfileDto;
    final groups = results[1] as List<TimelineGroupDto>;
    // 载入「待确认」集(build 里同步判断 isPending 前要先加载好)。
    await ReviewState.instance.ensureLoaded();
    // 兜底自动命名:示例数据等不走导入流程的路径,也能把默认档案改成识别到的姓名。
    await autoNameCurrentProfileFrom(profile.name);
    // 回填当前成员记录数,设置页据此展示每人多少份(不必逐个开库去数)。
    await ProfileManager.instance.setCount(
      ProfileManager.instance.current,
      profile.recordCount,
    );
    return (profile, groups);
  }

  /// 删除前确认(销毁性操作)。返回用户是否确认。
  Future<bool> _confirmDelete(String what) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('删除这份记录?'),
        content: Text('「$what」将从健康档案移除,此操作不可撤销。'),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('取消'),
          ),
          TextButton(
            style: TextButton.styleFrom(foregroundColor: MedMe.danger),
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('删除'),
          ),
        ],
      ),
    );
    return ok ?? false;
  }

  /// 删除一份文档:调 FFI(追加删除事件 + 重放),清掉可能的「待确认」标记,刷新档案。
  Future<void> _delete(int docId) async {
    try {
      await deleteDocument(documentId: docId);
      await ReviewState.instance.markReviewed(docId);
      bumpVaultRevision();
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text('删除失败:$e')));
      }
    }
  }

  /// 确认后删除(供 review 卡按钮 / 时间线左滑复用)。
  Future<void> _confirmAndDelete(int docId, String label) async {
    if (await _confirmDelete(label)) await _delete(docId);
  }

  /// 顶部 banner 点击:弹出成员切换器(家庭多成员)。
  Future<void> _showProfileSwitcher() async {
    await ProfileManager.instance.ensureLoaded();
    final members = ProfileManager.instance.members;
    final current = ProfileManager.instance.current;
    if (!mounted) return;
    final action = await showModalBottomSheet<String>(
      context: context,
      showDragHandle: true,
      builder: (context) => SafeArea(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Padding(
              padding: EdgeInsets.fromLTRB(20, 4, 20, 8),
              child: Align(
                alignment: Alignment.centerLeft,
                child: Text(
                  '切换成员',
                  style: TextStyle(fontSize: 16, fontWeight: FontWeight.w700),
                ),
              ),
            ),
            for (final m in members)
              ListTile(
                leading: CircleAvatar(
                  backgroundColor: MedMe.tealSoft,
                  child: Text(
                    m.isNotEmpty ? m[0] : '?',
                    style: const TextStyle(
                      color: MedMe.teal,
                      fontWeight: FontWeight.w700,
                    ),
                  ),
                ),
                title: Text(
                  m,
                  style: const TextStyle(fontWeight: FontWeight.w600),
                ),
                trailing: m == current
                    ? const Icon(Icons.check, color: MedMe.teal)
                    : null,
                onTap: () => Navigator.of(context).pop('member:$m'),
              ),
            const Divider(height: 1),
            ListTile(
              leading: const Icon(Icons.person_add_alt, color: MedMe.teal),
              title: const Text('添加成员'),
              onTap: () => Navigator.of(context).pop('add'),
            ),
            const SizedBox(height: 8),
          ],
        ),
      ),
    );
    if (action == null || !mounted) return;
    if (action == 'add') {
      await _addMember();
    } else if (action.startsWith('member:')) {
      final name = action.substring('member:'.length);
      if (name != current) {
        await switchProfileAndReopen(name);
        if (mounted) setState(() {});
      }
    }
  }

  Future<void> _addMember() async {
    final controller = TextEditingController();
    final name = await showDialog<String>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('添加成员'),
        content: TextField(
          controller: controller,
          autofocus: true,
          decoration: const InputDecoration(hintText: '输入姓名(家庭内唯一即可)'),
          onSubmitted: (v) => Navigator.of(context).pop(v),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(controller.text),
            child: const Text('创建'),
          ),
        ],
      ),
    );
    if (name == null || name.trim().isEmpty || !mounted) return;
    await createProfileAndReopen(name.trim());
    if (mounted) setState(() {});
  }

  Future<void> _refresh() async {
    final next = _load();
    setState(() => _future = next);
    await next;
  }

  void _openDoc(int id) {
    Navigator.of(
      context,
    ).push(MaterialPageRoute(builder: (_) => DocumentDetailScreen(docId: id)));
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('健康档案'),
        actions: [
          // 右上角「导入」:弹三选一(拍照/相册/选文件),导入后本屏经 vaultRevision 自动刷新。
          Padding(
            padding: const EdgeInsets.only(right: 8),
            child: TextButton.icon(
              onPressed: () => showImportSheet(context),
              icon: const Icon(Icons.add, size: 20),
              label: const Text('导入'),
            ),
          ),
        ],
      ),
      body: FutureBuilder<(PatientProfileDto, List<TimelineGroupDto>)>(
        future: _future,
        builder: (context, snap) {
          if (snap.connectionState != ConnectionState.done) {
            return const Center(
              child: CircularProgressIndicator(color: MedMe.teal),
            );
          }
          if (snap.hasError) {
            return RefreshIndicator(
              onRefresh: _refresh,
              child: ListView(
                physics: const AlwaysScrollableScrollPhysics(),
                children: [
                  Padding(
                    padding: const EdgeInsets.all(32),
                    child: Text(
                      '加载健康档案失败:\n${snap.error}\n\n下拉可重试。',
                      textAlign: TextAlign.center,
                      style: const TextStyle(color: MedMe.faint),
                    ),
                  ),
                ],
              ),
            );
          }

          final (profile, groups) = snap.data!;
          // 待确认(新导入)文档:红框置顶,新的(id 大)在前;确认在详情页做。
          final pending =
              _allDocs(
                  groups,
                ).where((d) => ReviewState.instance.isPending(d.id)).toList()
                ..sort((a, b) => b.id.compareTo(a.id));
          // 已确认时间线:剔除待确认文档,避免和上面红框区重复。
          final confirmed = _confirmedOnly(groups);
          return RefreshIndicator(
            onRefresh: _refresh,
            color: MedMe.teal,
            child: ListView(
              physics: const AlwaysScrollableScrollPhysics(),
              padding: const EdgeInsets.fromLTRB(16, 16, 16, 32),
              children: [
                _PatientHeader(
                  profile: profile,
                  memberName: ProfileManager.instance.displayName,
                  onTap: _showProfileSwitcher,
                ),
                const SizedBox(height: 20),
                // 待确认:红框卡片,点开进详情核对 + 确认;左滑删除。
                for (final d in pending) ...[
                  _PendingCard(
                    doc: d,
                    mismatchName: ReviewState.instance.mismatchName(d.id),
                    onOpen: _openDoc,
                    onDelete: _confirmAndDelete,
                  ),
                  const SizedBox(height: 10),
                ],
                if (pending.isNotEmpty && confirmed.isNotEmpty) ...[
                  const SizedBox(height: 6),
                  Row(
                    children: [
                      const Expanded(child: Divider(color: MedMe.line)),
                      Padding(
                        padding: const EdgeInsets.symmetric(horizontal: 10),
                        child: Text(
                          '以下为已确认',
                          style: TextStyle(fontSize: 12, color: MedMe.faint),
                        ),
                      ),
                      const Expanded(child: Divider(color: MedMe.line)),
                    ],
                  ),
                  const SizedBox(height: 10),
                ],
                if (pending.isEmpty && confirmed.isEmpty)
                  const _EmptyState()
                else
                  for (var i = 0; i < confirmed.length; i++) ...[
                    if (i > 0) const SizedBox(height: 10),
                    _TimelineItem(
                      group: confirmed[i],
                      // 按就诊组 id 记展开态(不用列表下标)——删除/导入后下标会错位到别的组。
                      expanded: switch (confirmed[i]) {
                        TimelineGroupDto_Encounter(:final encounter) =>
                          _expanded.contains(encounter.id),
                        _ => false,
                      },
                      onTap: () {
                        switch (confirmed[i]) {
                          case TimelineGroupDto_Document(:final doc):
                            _openDoc(doc.id);
                          case TimelineGroupDto_Encounter(:final encounter):
                            setState(() {
                              if (!_expanded.add(encounter.id)) {
                                _expanded.remove(encounter.id);
                              }
                            });
                        }
                      },
                      onOpenSubDoc: _openDoc,
                      onDelete: _confirmAndDelete,
                    ),
                  ],
              ],
            ),
          );
        },
      ),
    );
  }
}

/// 患者头卡:姓名 / 性别·年龄 / 记录数,字段可空一律优雅缺省。
class _PatientHeader extends StatelessWidget {
  final PatientProfileDto profile;
  final String memberName;
  final VoidCallback onTap;
  const _PatientHeader({
    required this.profile,
    required this.memberName,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final initial = memberName.isNotEmpty ? memberName[0] : '我';
    final subParts = [
      profile.gender,
      profile.age,
    ].whereType<String>().where((s) => s.isNotEmpty).toList();
    subParts.add('${profile.recordCount} 份记录');

    return Material(
      color: MedMe.panel,
      borderRadius: BorderRadius.circular(16),
      child: InkWell(
        onTap: onTap, // 点顶部切换成员(家庭多成员)
        borderRadius: BorderRadius.circular(16),
        child: Container(
          padding: const EdgeInsets.all(16),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(16),
            border: Border.all(color: MedMe.line),
          ),
          child: Row(
            children: [
              CircleAvatar(
                radius: 26,
                backgroundColor: MedMe.tealSoft,
                child: Text(
                  initial,
                  style: const TextStyle(
                    color: MedMe.teal,
                    fontWeight: FontWeight.w700,
                    fontSize: 18,
                  ),
                ),
              ),
              const SizedBox(width: 14),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      children: [
                        Flexible(
                          child: Text(
                            memberName,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: const TextStyle(
                              fontSize: 17,
                              fontWeight: FontWeight.w800,
                              color: MedMe.ink,
                            ),
                          ),
                        ),
                        const SizedBox(width: 4),
                        const Icon(
                          Icons.unfold_more,
                          size: 18,
                          color: MedMe.faint,
                        ),
                      ],
                    ),
                    const SizedBox(height: 2),
                    Text(
                      subParts.join(' · '),
                      style: const TextStyle(
                        fontSize: 12.5,
                        color: MedMe.faint,
                      ),
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// 空态引导:没有记录时提示点右上角「导入」,或去「设置」载入示例数据。
class _EmptyState extends StatelessWidget {
  const _EmptyState();

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 56),
      child: Column(
        children: [
          const Icon(Icons.folder_outlined, size: 48, color: MedMe.faint),
          const SizedBox(height: 14),
          const Text(
            '还没有病历',
            style: TextStyle(
              fontSize: 15,
              fontWeight: FontWeight.w600,
              color: MedMe.ink,
            ),
          ),
          const SizedBox(height: 6),
          const Text(
            '点右上角「导入」拍照或选择文件添加,\n或在「设置」里载入示例数据试试看',
            textAlign: TextAlign.center,
            style: TextStyle(fontSize: 13, color: MedMe.faint, height: 1.6),
          ),
        ],
      ),
    );
  }
}

/// 时间线一项:就诊组(可展开子文档)或独立文档。
/// 时间线/待确认项左滑删除时的红底背景(靠右露出删除图标),Outlook 邮件式。
Widget swipeDeleteBackground() => Container(
  alignment: Alignment.centerRight,
  padding: const EdgeInsets.symmetric(horizontal: 20),
  decoration: BoxDecoration(
    color: MedMe.danger,
    borderRadius: BorderRadius.circular(14),
  ),
  child: const Icon(Icons.delete_outline, color: Colors.white),
);

class _TimelineItem extends StatelessWidget {
  final TimelineGroupDto group;
  final bool expanded;
  final VoidCallback onTap;
  final void Function(int docId) onOpenSubDoc;
  final Future<void> Function(int docId, String label) onDelete;

  const _TimelineItem({
    required this.group,
    required this.expanded,
    required this.onTap,
    required this.onOpenSubDoc,
    required this.onDelete,
  });

  @override
  Widget build(BuildContext context) {
    final isEncounter = group is TimelineGroupDto_Encounter;
    final colorKey = switch (group) {
      TimelineGroupDto_Encounter() => 'enc',
      TimelineGroupDto_Document(:final doc) => doc.docType,
    };
    final color = _colorFor(colorKey);
    final icon = switch (group) {
      TimelineGroupDto_Encounter(:final encounter) => _iconForKind(
        encounter.kind,
      ),
      TimelineGroupDto_Document(:final doc) => _iconForDoc(doc.docType),
    };

    final Widget card = Container(
      decoration: BoxDecoration(
        color: MedMe.panel,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: MedMe.line),
      ),
      clipBehavior: Clip.antiAlias,
      child: Column(
        children: [
          InkWell(
            onTap: onTap,
            child: Padding(
              padding: const EdgeInsets.all(12),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Container(
                    width: 36,
                    height: 36,
                    alignment: Alignment.center,
                    decoration: BoxDecoration(
                      color: color.withValues(alpha: 0.1),
                      borderRadius: BorderRadius.circular(10),
                    ),
                    child: Icon(icon, size: 19, color: color),
                  ),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Row(
                          children: [
                            Expanded(
                              child: Text(
                                _groupTitle(group),
                                style: const TextStyle(
                                  fontSize: 14.5,
                                  fontWeight: FontWeight.w700,
                                ),
                                overflow: TextOverflow.ellipsis,
                              ),
                            ),
                            const SizedBox(width: 8),
                            Text(
                              _groupDate(group),
                              style: const TextStyle(
                                fontSize: 12,
                                color: MedMe.faint,
                              ),
                            ),
                          ],
                        ),
                        const SizedBox(height: 3),
                        Text(
                          _groupDesc(group),
                          style: const TextStyle(
                            fontSize: 12.5,
                            color: MedMe.faint,
                          ),
                        ),
                      ],
                    ),
                  ),
                  if (isEncounter)
                    Icon(
                      expanded ? Icons.expand_less : Icons.expand_more,
                      size: 20,
                      color: MedMe.faint,
                    ),
                ],
              ),
            ),
          ),
          if (expanded)
            switch (group) {
              TimelineGroupDto_Encounter(:final docs) => _SubDocList(
                docs: docs,
                onOpenSubDoc: onOpenSubDoc,
                onDelete: onDelete,
              ),
              TimelineGroupDto_Document() => const SizedBox.shrink(),
            },
        ],
      ),
    );

    // 独立文档项:左滑删除(Outlook 式)。就诊组不整组删——展开后删组内单份。
    if (group case TimelineGroupDto_Document(:final doc)) {
      return Dismissible(
        key: ValueKey('tl-doc-${doc.id}'),
        direction: DismissDirection.endToStart,
        background: swipeDeleteBackground(),
        confirmDismiss: (_) async {
          await onDelete(doc.id, _groupTitle(group));
          return false; // 由数据重载移除,避免与 Dismissible 自身移除冲突
        },
        child: card,
      );
    }
    return card;
  }
}

class _SubDocList extends StatelessWidget {
  final List<DocumentSummaryDto> docs;
  final void Function(int docId) onOpenSubDoc;
  final Future<void> Function(int docId, String label) onDelete;

  const _SubDocList({
    required this.docs,
    required this.onOpenSubDoc,
    required this.onDelete,
  });

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        for (final d in docs)
          Dismissible(
            key: ValueKey('sub-doc-${d.id}'),
            direction: DismissDirection.endToStart,
            background: swipeDeleteBackground(),
            confirmDismiss: (_) async {
              await onDelete(d.id, d.title ?? _docLabel[d.docType] ?? '记录');
              return false;
            },
            child: Container(
            decoration: const BoxDecoration(
              border: Border(top: BorderSide(color: MedMe.line)),
            ),
            child: InkWell(
              onTap: () => onOpenSubDoc(d.id),
              child: Padding(
                padding: const EdgeInsets.symmetric(
                  horizontal: 12,
                  vertical: 10,
                ),
                child: Row(
                  children: [
                    Container(
                      width: 26,
                      height: 26,
                      alignment: Alignment.center,
                      decoration: BoxDecoration(
                        color: _colorFor(d.docType).withValues(alpha: 0.1),
                        borderRadius: BorderRadius.circular(8),
                      ),
                      child: Icon(
                        _iconForDoc(d.docType),
                        size: 14,
                        color: _colorFor(d.docType),
                      ),
                    ),
                    const SizedBox(width: 10),
                    Expanded(
                      child: Text(
                        d.title ?? _docLabel[d.docType] ?? '记录',
                        style: const TextStyle(fontSize: 13),
                        overflow: TextOverflow.ellipsis,
                      ),
                    ),
                    Text(
                      _fmtDate(d.docDate),
                      style: const TextStyle(
                        fontSize: 11.5,
                        color: MedMe.faint,
                      ),
                    ),
                  ],
                ),
              ),
            ),
            ),
          ),
      ],
    );
  }
}

/// 待确认(新导入)卡片:红框 + 「待确认」标签,点开进**详情页**核对并确认(确认按钮
/// 在详情页,不在这里)。左滑删除。识别姓名与当前档案不符时下方红色警告。确认后本卡
/// 消失,该文档以标准样式进入下方时间线。
class _PendingCard extends StatelessWidget {
  const _PendingCard({
    required this.doc,
    required this.mismatchName,
    required this.onOpen,
    required this.onDelete,
  });

  final DocumentSummaryDto doc;
  final String? mismatchName;
  final void Function(int docId) onOpen;
  final Future<void> Function(int docId, String label) onDelete;

  @override
  Widget build(BuildContext context) {
    final label = doc.title ?? _docLabel[doc.docType] ?? '记录';
    final card = Container(
      decoration: BoxDecoration(
        color: MedMe.panel,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: MedMe.danger, width: 1.5),
      ),
      clipBehavior: Clip.antiAlias,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          InkWell(
            onTap: () => onOpen(doc.id),
            child: Padding(
              padding: const EdgeInsets.all(12),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Container(
                    width: 36,
                    height: 36,
                    alignment: Alignment.center,
                    decoration: BoxDecoration(
                      color: _colorFor(doc.docType).withValues(alpha: 0.1),
                      borderRadius: BorderRadius.circular(10),
                    ),
                    child: Icon(
                      _iconForDoc(doc.docType),
                      size: 19,
                      color: _colorFor(doc.docType),
                    ),
                  ),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Row(
                          children: [
                            Container(
                              padding: const EdgeInsets.symmetric(
                                horizontal: 7,
                                vertical: 2,
                              ),
                              decoration: BoxDecoration(
                                color: MedMe.danger.withValues(alpha: 0.1),
                                borderRadius: BorderRadius.circular(6),
                              ),
                              child: const Text(
                                '待确认',
                                style: TextStyle(
                                  fontSize: 11,
                                  fontWeight: FontWeight.w700,
                                  color: MedMe.danger,
                                ),
                              ),
                            ),
                            const SizedBox(width: 8),
                            Expanded(
                              child: Text(
                                label,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: const TextStyle(
                                  fontSize: 14.5,
                                  fontWeight: FontWeight.w700,
                                ),
                              ),
                            ),
                            const SizedBox(width: 8),
                            Text(
                              _fmtDate(doc.docDate),
                              style: const TextStyle(
                                fontSize: 12,
                                color: MedMe.faint,
                              ),
                            ),
                          ],
                        ),
                        const SizedBox(height: 3),
                        Text(
                          [_docLabel[doc.docType] ?? doc.docType, '点开核对并确认']
                              .join(' · '),
                          style: const TextStyle(
                            fontSize: 12.5,
                            color: MedMe.faint,
                          ),
                        ),
                      ],
                    ),
                  ),
                  const Icon(Icons.chevron_right, size: 20, color: MedMe.faint),
                ],
              ),
            ),
          ),
          if (mismatchName case final who?) _MismatchBanner(who: who),
        ],
      ),
    );
    return Dismissible(
      key: ValueKey('pending-${doc.id}'),
      direction: DismissDirection.endToStart,
      background: swipeDeleteBackground(),
      confirmDismiss: (_) async {
        await onDelete(doc.id, label);
        return false;
      },
      child: card,
    );
  }
}

/// 这份报告识别到的患者姓名和当前档案不一致 → 醒目提示,可能导错了人。
/// 只警告不自动搬(用户可自行处理);点开核对无误后「确认」即可归档。
class _MismatchBanner extends StatelessWidget {
  const _MismatchBanner({required this.who});

  final String who;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: double.infinity,
      margin: const EdgeInsets.fromLTRB(12, 0, 12, 12),
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
      decoration: BoxDecoration(
        color: Colors.orange.withValues(alpha: 0.10),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: Colors.orange.withValues(alpha: 0.4)),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Icon(
            Icons.warning_amber_rounded,
            color: Colors.orange,
            size: 18,
          ),
          const SizedBox(width: 6),
          Expanded(
            child: Text(
              '报告上的姓名是「$who」,与当前档案「${ProfileManager.instance.current}」不一致,'
              '请核对是否导错了人。',
              style: const TextStyle(
                fontSize: 12,
                height: 1.4,
                color: Color(0xFFB25E00),
              ),
            ),
          ),
        ],
      ),
    );
  }
}
