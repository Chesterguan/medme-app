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
      final parts = ['${encounter.docCount} 份记录', ...kinds.take(3)];
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

class ArchiveScreen extends StatefulWidget {
  const ArchiveScreen({super.key});

  @override
  State<ArchiveScreen> createState() => _ArchiveScreenState();
}

class _ArchiveScreenState extends State<ArchiveScreen> {
  late Future<(PatientProfileDto, List<TimelineGroupDto>)> _future = _load();
  // 就诊组在时间线里点开时展开其子文档(可再点开各自详情)。
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
    return (profile, groups);
  }

  /// 审核通过一份文档 → 从顶部「待确认」区消失,归入正常时间线(按日期排序)。
  Future<void> _review(int docId) async {
    await ReviewState.instance.markReviewed(docId);
    if (mounted) setState(() {}); // 重建 → 重算 unreviewed(数据没变,只是过滤变了)
  }

  Future<void> _reviewAll(Iterable<int> docIds) async {
    await ReviewState.instance.markAllReviewed(docIds);
    if (mounted) setState(() {});
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
          // 新导入(待确认)的文档:置顶,新的(id 大)在前。
          final unreviewed =
              _allDocs(
                  groups,
                ).where((d) => ReviewState.instance.isPending(d.id)).toList()
                ..sort((a, b) => b.id.compareTo(a.id));
          return RefreshIndicator(
            onRefresh: _refresh,
            color: MedMe.teal,
            child: ListView(
              physics: const AlwaysScrollableScrollPhysics(),
              padding: const EdgeInsets.fromLTRB(16, 16, 16, 32),
              children: [
                _PatientHeader(
                  profile: profile,
                  memberName: ProfileManager.instance.current,
                  onTap: _showProfileSwitcher,
                ),
                const SizedBox(height: 20),
                if (unreviewed.isNotEmpty) ...[
                  _NewImportsSection(
                    docs: unreviewed,
                    onReview: _review,
                    onReviewAll: () => _reviewAll(unreviewed.map((d) => d.id)),
                    onOpen: _openDoc,
                  ),
                  const SizedBox(height: 20),
                ],
                if (groups.isEmpty)
                  const _EmptyState()
                else
                  for (var i = 0; i < groups.length; i++) ...[
                    if (i > 0) const SizedBox(height: 10),
                    _TimelineItem(
                      group: groups[i],
                      expanded: _expanded.contains(i),
                      onTap: () {
                        final g = groups[i];
                        if (g is TimelineGroupDto_Document) {
                          _openDoc(g.doc.id);
                        } else {
                          setState(() {
                            if (!_expanded.add(i)) _expanded.remove(i);
                          });
                        }
                      },
                      onOpenSubDoc: _openDoc,
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

/// 空态引导:没有记录时提示去「导入导出」或「设置」载入示例数据。
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
            '去「导入导出」拍照或选择文件添加,\n或在「设置」里载入示例数据试试看',
            textAlign: TextAlign.center,
            style: TextStyle(fontSize: 13, color: MedMe.faint, height: 1.6),
          ),
        ],
      ),
    );
  }
}

/// 时间线一项:就诊组(可展开子文档)或独立文档。
class _TimelineItem extends StatelessWidget {
  final TimelineGroupDto group;
  final bool expanded;
  final VoidCallback onTap;
  final void Function(int docId) onOpenSubDoc;

  const _TimelineItem({
    required this.group,
    required this.expanded,
    required this.onTap,
    required this.onOpenSubDoc,
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

    return Container(
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
              ),
              TimelineGroupDto_Document() => const SizedBox.shrink(),
            },
        ],
      ),
    );
  }
}

class _SubDocList extends StatelessWidget {
  final List<DocumentSummaryDto> docs;
  final void Function(int docId) onOpenSubDoc;

  const _SubDocList({required this.docs, required this.onOpenSubDoc});

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        for (final d in docs)
          Container(
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
      ],
    );
  }
}

/// 顶部「待确认 · 新导入」区:新导入的文档先在这里让用户过一眼(OCR 猜的类型/日期
/// 可能不准),点「确认」→ 归入下方正常时间线(按日期排序)、不再置顶。
class _NewImportsSection extends StatelessWidget {
  const _NewImportsSection({
    required this.docs,
    required this.onReview,
    required this.onReviewAll,
    required this.onOpen,
  });

  final List<DocumentSummaryDto> docs;
  final void Function(int docId) onReview;
  final VoidCallback onReviewAll;
  final void Function(int docId) onOpen;

  @override
  Widget build(BuildContext context) {
    return Container(
      decoration: BoxDecoration(
        color: MedMe.tealSoft,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: MedMe.teal.withValues(alpha: 0.3)),
      ),
      padding: const EdgeInsets.fromLTRB(12, 10, 12, 12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              const Icon(Icons.fiber_new, color: MedMe.teal, size: 20),
              const SizedBox(width: 6),
              Text(
                '待确认 · 新导入 ${docs.length} 份',
                style: const TextStyle(
                  fontSize: 14,
                  fontWeight: FontWeight.w700,
                  color: MedMe.tealDark,
                ),
              ),
              const Spacer(),
              if (docs.length > 1)
                TextButton(onPressed: onReviewAll, child: const Text('全部确认')),
            ],
          ),
          const Padding(
            padding: EdgeInsets.only(left: 26, right: 4, bottom: 4),
            child: Text(
              '识别的类型/日期可能不准,点开核对无误后「确认」,即归入下方时间线。',
              style: TextStyle(fontSize: 12, color: MedMe.faint, height: 1.4),
            ),
          ),
          for (final d in docs)
            Card(
              margin: const EdgeInsets.only(top: 8),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  ListTile(
                    leading: CircleAvatar(
                      backgroundColor: _colorFor(
                        d.docType,
                      ).withValues(alpha: 0.12),
                      child: Icon(
                        _iconForDoc(d.docType),
                        color: _colorFor(d.docType),
                        size: 20,
                      ),
                    ),
                    title: Text(
                      d.title ?? _docLabel[d.docType] ?? '记录',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(fontWeight: FontWeight.w600),
                    ),
                    subtitle: Text(
                      [
                        _docLabel[d.docType] ?? d.docType,
                        _fmtDate(d.docDate).isNotEmpty
                            ? _fmtDate(d.docDate)
                            : '无日期',
                      ].join(' · '),
                      style: const TextStyle(
                        color: MedMe.faint,
                        fontSize: 12.5,
                      ),
                    ),
                    trailing: FilledButton(
                      style: FilledButton.styleFrom(
                        visualDensity: VisualDensity.compact,
                      ),
                      onPressed: () => onReview(d.id),
                      child: const Text('确认'),
                    ),
                    onTap: () => onOpen(d.id),
                  ),
                  if (ReviewState.instance.mismatchName(d.id) case final who?)
                    _MismatchBanner(who: who),
                ],
              ),
            ),
        ],
      ),
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
