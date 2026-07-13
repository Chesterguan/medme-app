import 'package:mobile_flutter/src/rust/api/dto.dart';

/// 「导入导出」屏的纯逻辑小工具:与 Widget 树无关,方便单独看清楚。
/// 文档类型中文标签,与桌面 / 旧 Tauri 移动端 `DOC_LABEL` 保持一致
/// (见 `apps/mobile/src/App.tsx`),汇总弹窗里「归类为」文案用它。
const Map<String, String> kDocTypeLabel = {
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

/// 图片扩展名:拍照 / 相册天然是图片;「选择文件」里靠这些后缀判断是否也走
/// OCR 通道(与 Rust 端 `pipeline::mime_for` 按扩展名判 MIME 的思路一致)。
const Set<String> kImageExtensions = {'png', 'jpg', 'jpeg', 'tiff', 'heic'};

/// 按文件名后缀判断是否为图片(大小写不敏感)。
bool isImageName(String name) {
  final dot = name.lastIndexOf('.');
  if (dot < 0 || dot == name.length - 1) return false;
  return kImageExtensions.contains(name.substring(dot + 1).toLowerCase());
}

/// 一份待导入项:统一拍照 / 相册 / 文件选择器三种来源——三者在设备上都有
/// 真实文件路径(仅移动端,不考虑 web),据此读字节、跑 OCR。
class PendingImport {
  final String name;
  final String path;
  final bool isImage;

  const PendingImport({
    required this.name,
    required this.path,
    required this.isImage,
  });
}

/// 单份文件导入结果的展示态:区分「FFI 落库但状态非全新成功」与
/// 「处理过程中直接抛异常」,汇总弹窗按此分类计数。
enum ImportRowKind { success, duplicate, storedNoText, failed }

class ImportResultRow {
  final String name;
  final String statusLabel;
  final ImportRowKind kind;

  const ImportResultRow({
    required this.name,
    required this.statusLabel,
    required this.kind,
  });
}

/// 把 `ImportOutcomeDto.status`(见 rust/src/api/dto.rs 注释:
/// new|backfilled|deduped|stored_no_text|instance_attached|failed)映射成
/// 老人能看懂的一行结果。
ImportResultRow rowFromOutcome(ImportOutcomeDto outcome) {
  final typeLabel = outcome.docType == null
      ? null
      : (kDocTypeLabel[outcome.docType] ?? outcome.docType);
  switch (outcome.status) {
    case 'new':
    case 'backfilled':
    case 'instance_attached':
      return ImportResultRow(
        name: outcome.name,
        statusLabel: typeLabel != null ? '已识别入库 · $typeLabel' : '已识别入库',
        kind: ImportRowKind.success,
      );
    case 'deduped':
      return ImportResultRow(
        name: outcome.name,
        statusLabel: '重复,已跳过',
        kind: ImportRowKind.duplicate,
      );
    case 'stored_no_text':
      return ImportResultRow(
        name: outcome.name,
        statusLabel: '仅存原件(未识别到文字)',
        kind: ImportRowKind.storedNoText,
      );
    default:
      return ImportResultRow(
        name: outcome.name,
        statusLabel: '未能处理',
        kind: ImportRowKind.failed,
      );
  }
}

/// 单份文件处理过程中直接抛异常(读文件失败、FFI 报错等),同样归入失败展示行,
/// 不让一份文件的问题中断整个批次。
ImportResultRow rowFromError(String name, Object error) => ImportResultRow(
  name: name,
  statusLabel: '导入失败:$error',
  kind: ImportRowKind.failed,
);
