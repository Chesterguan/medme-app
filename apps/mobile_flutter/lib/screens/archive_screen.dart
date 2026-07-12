import 'package:flutter/material.dart';

import 'package:mobile_flutter/src/rust/api/dto.dart';
import 'package:mobile_flutter/src/rust/api/vault.dart';
import 'package:mobile_flutter/theme.dart';
import 'package:mobile_flutter/screens/document_detail.dart';

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

class ArchiveScreen extends StatefulWidget {
  const ArchiveScreen({super.key});

  @override
  State<ArchiveScreen> createState() => _ArchiveScreenState();
}

class _ArchiveScreenState extends State<ArchiveScreen> {
  late Future<(PatientProfileDto, List<TimelineGroupDto>)> _future = _load();
  // 就诊组在时间线里点开时展开其子文档(可再点开各自详情)。
  final Set<int> _expanded = {};

  Future<(PatientProfileDto, List<TimelineGroupDto>)> _load() async {
    final results = await Future.wait([patientProfile(), loadArchive()]);
    return (
      results[0] as PatientProfileDto,
      results[1] as List<TimelineGroupDto>,
    );
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
      appBar: AppBar(title: const Text('健康档案')),
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
          return RefreshIndicator(
            onRefresh: _refresh,
            color: MedMe.teal,
            child: ListView(
              physics: const AlwaysScrollableScrollPhysics(),
              padding: const EdgeInsets.fromLTRB(16, 16, 16, 32),
              children: [
                _PatientHeader(profile: profile),
                const SizedBox(height: 20),
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
  const _PatientHeader({required this.profile});

  @override
  Widget build(BuildContext context) {
    final initial = (profile.name?.isNotEmpty ?? false)
        ? profile.name![0]
        : '我';
    final subParts = [
      profile.gender,
      profile.age,
    ].whereType<String>().where((s) => s.isNotEmpty).toList();
    subParts.add('${profile.recordCount} 份记录');

    return Container(
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: MedMe.panel,
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
                Text(
                  profile.name ?? '我的健康档案',
                  style: const TextStyle(
                    fontSize: 17,
                    fontWeight: FontWeight.w800,
                    color: MedMe.ink,
                  ),
                ),
                const SizedBox(height: 2),
                Text(
                  subParts.join(' · '),
                  style: const TextStyle(fontSize: 12.5, color: MedMe.faint),
                ),
              ],
            ),
          ),
        ],
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
