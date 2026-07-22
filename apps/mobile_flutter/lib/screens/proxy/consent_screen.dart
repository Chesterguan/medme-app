import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:signature/signature.dart';

import 'package:mobile_flutter/src/rust/api/dto.dart';
import 'package:mobile_flutter/theme.dart';

/// 同意告知文案的版本号。文案改了就升这个号——落进加密包(`ConsentDto.consentTextVersion`),
/// 便于日后区分「病人是在哪版文案下同意的」。
const String kConsentTextVersion = 'v1';

/// 拍前同意大屏(医生代拍病人纸质材料流程的**第一屏、任何采集之前**)。大字白话
/// 告知:拍什么 · 为什么 · 给谁 · 存多久 · 我们解不开 · 你能删能撤。病人签名确认;
/// 签不了字时「按住 3 秒确认」兜底(画押式手势,不用写字)。
///
/// 产出 [ConsentDto] 经 [onAgreed] 交给外层流程——本屏自己不碰 Rust FFI,只负责
/// 采集「谁、以何种方式、何时同意」这一件事。
class ConsentScreen extends StatefulWidget {
  const ConsentScreen({super.key, required this.onAgreed, required this.onCancel});

  final ValueChanged<ConsentDto> onAgreed;
  final VoidCallback onCancel;

  @override
  State<ConsentScreen> createState() => _ConsentScreenState();
}

class _ConsentScreenState extends State<ConsentScreen>
    with SingleTickerProviderStateMixin {
  late final SignatureController _sigController = SignatureController(
    penStrokeWidth: 3,
    penColor: MedMe.ink,
    exportBackgroundColor: Colors.white,
  );
  late final AnimationController _holdController = AnimationController(
    vsync: this,
    duration: const Duration(seconds: 3),
  );
  // 本次代建档会话的人类可读标识(落进 ConsentDto.sessionId,供医生/病人事后核对
  // 「哪一次代拍」;不是安全边界——临时会话本身的随机 device_id 才是,见 ephemeral.rs)。
  late final String _sessionId =
      'sess-${DateTime.now().millisecondsSinceEpoch}-${Random().nextInt(0xFFFFFF).toRadixString(16)}';

  bool _useSignature = true;
  bool _submitting = false;
  // 一次性开关,与 `_submitting`(签名提交中的 UI 忙态)分开:签名按钮已被
  // `_submitting` 挡了重复点击,但按住确认手势没有——若用户在签名提交的 await
  // 期间又按住满 3 秒,会触发第二次 `onAgreed` → 第二次 `ephemeral_begin`,把
  // 第一个(空)会话晾在那儿。两条路径都先查这个再往下走,保证 onAgreed 全程
  // 只触发一次。
  bool _confirmed = false;

  @override
  void initState() {
    super.initState();
    _holdController.addStatusListener((status) {
      if (status == AnimationStatus.completed) {
        _emit(method: 'press_hold');
      }
    });
  }

  @override
  void dispose() {
    _sigController.dispose();
    _holdController.dispose();
    super.dispose();
  }

  Future<void> _confirmWithSignature() async {
    if (_sigController.isEmpty) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(const SnackBar(content: Text('请先在下方签名')));
      return;
    }
    setState(() => _submitting = true);
    final Uint8List? png = await _sigController.toPngBytes();
    if (!mounted) return;
    if (png == null) {
      setState(() => _submitting = false);
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(const SnackBar(content: Text('签名保存失败,请重试')));
      return;
    }
    _emit(method: 'signature', signaturePngBase64: base64Encode(png));
  }

  void _emit({required String method, String? signaturePngBase64}) {
    if (_confirmed) return; // 见字段声明处的说明:保证 onAgreed 全程只触发一次。
    _confirmed = true;
    widget.onAgreed(
      ConsentDto(
        utcTs: DateTime.now().toUtc().toIso8601String(),
        consentTextVersion: kConsentTextVersion,
        signaturePngBase64: signaturePngBase64,
        method: method,
        sessionId: _sessionId,
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    // 不是独立 Scaffold——composes 进 `ProxyIntakeFlow` 的 Scaffold(顶部常驻橙色
    // 横幅在外层,任何阶段都在),这里只是内容区。
    return ColoredBox(
      color: MedMe.bg,
      child: SafeArea(
        top: false,
        child: SingleChildScrollView(
          padding: const EdgeInsets.fromLTRB(20, 16, 20, 24),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              const Icon(
                Icons.privacy_tip_outlined,
                color: MedMe.proxyOrange,
                size: 44,
              ),
              const SizedBox(height: 14),
              const Text(
                '在拍之前,请告诉对方这几件事',
                style: TextStyle(fontSize: 21, fontWeight: FontWeight.w800),
              ),
              const SizedBox(height: 18),
              const _ConsentPoint(
                icon: Icons.camera_alt_outlined,
                title: '拍什么',
                body: '接下来会拍下您的化验单、处方、检查报告等纸质病历材料。',
              ),
              const _ConsentPoint(
                icon: Icons.favorite_border,
                title: '为什么',
                body: '帮您把这些纸质材料整理成一份电子病历,方便您以后就诊、复查时携带。',
              ),
              const _ConsentPoint(
                icon: Icons.person_outline,
                title: '给谁',
                body: '只生成一份加密文件和一串口令,交给您本人保管;不会自动发给任何人。',
              ),
              const _ConsentPoint(
                icon: Icons.schedule_outlined,
                title: '存多久',
                body: '这台设备上不会留底 —— 文件交给您之后,拍摄用到的所有内容会立刻从这台设备上删除。',
              ),
              const _ConsentPoint(
                icon: Icons.lock_outline,
                title: '我们解不开',
                body: '文件用端到端加密保护,没有那串口令,包括我们在内任何人都打不开、看不到内容。',
              ),
              const _ConsentPoint(
                icon: Icons.undo_outlined,
                title: '您能删、能撤',
                body: '整个过程您可以随时喊停;喊停后已拍的内容会立刻删除,不会生成任何文件。',
              ),
              const SizedBox(height: 22),
              const Divider(height: 1, color: MedMe.line),
              const SizedBox(height: 18),
              Text(
                _useSignature ? '请在下方签名确认' : '请按住下方按钮 3 秒确认',
                style: const TextStyle(fontSize: 15, fontWeight: FontWeight.w700),
              ),
              const SizedBox(height: 10),
              if (_useSignature) ...[
                Container(
                  height: 180,
                  decoration: BoxDecoration(
                    color: Colors.white,
                    borderRadius: BorderRadius.circular(12),
                    border: Border.all(color: MedMe.line),
                  ),
                  clipBehavior: Clip.antiAlias,
                  child: Signature(controller: _sigController, backgroundColor: Colors.white),
                ),
                const SizedBox(height: 10),
                Row(
                  children: [
                    TextButton(
                      onPressed: _submitting ? null : () => _sigController.clear(),
                      child: const Text('重签'),
                    ),
                    const Spacer(),
                    TextButton(
                      onPressed: _submitting
                          ? null
                          : () => setState(() => _useSignature = false),
                      child: const Text('不方便签名?'),
                    ),
                  ],
                ),
                const SizedBox(height: 8),
                SizedBox(
                  height: 50,
                  child: FilledButton(
                    style: FilledButton.styleFrom(
                      backgroundColor: MedMe.proxyOrange,
                    ),
                    onPressed: _submitting ? null : _confirmWithSignature,
                    child: _submitting
                        ? const SizedBox(
                            width: 20,
                            height: 20,
                            child: CircularProgressIndicator(
                              strokeWidth: 2.5,
                              color: Colors.white,
                            ),
                          )
                        : const Text('已签名,同意开始'),
                  ),
                ),
              ] else ...[
                Text(
                  '手指按住不放,进度环转满一圈即视为同意确认',
                  style: TextStyle(fontSize: 13, color: MedMe.faint),
                ),
                const SizedBox(height: 14),
                Center(
                  child: GestureDetector(
                    onTapDown: (_) => _holdController.forward(from: 0),
                    onTapCancel: () => _holdController.reverse(),
                    onTapUp: (_) => _holdController.reverse(),
                    child: AnimatedBuilder(
                      animation: _holdController,
                      builder: (context, child) => SizedBox(
                        width: 120,
                        height: 120,
                        child: Stack(
                          alignment: Alignment.center,
                          children: [
                            SizedBox(
                              width: 120,
                              height: 120,
                              child: CircularProgressIndicator(
                                value: _holdController.value,
                                strokeWidth: 8,
                                backgroundColor: MedMe.line,
                                valueColor: const AlwaysStoppedAnimation(
                                  MedMe.proxyOrange,
                                ),
                              ),
                            ),
                            const Text(
                              '按住\n确认',
                              textAlign: TextAlign.center,
                              style: TextStyle(
                                fontWeight: FontWeight.w700,
                                color: MedMe.proxyOrange,
                              ),
                            ),
                          ],
                        ),
                      ),
                    ),
                  ),
                ),
                const SizedBox(height: 14),
                Center(
                  child: TextButton(
                    onPressed: () => setState(() => _useSignature = true),
                    child: const Text('改用签名'),
                  ),
                ),
              ],
              const SizedBox(height: 8),
              Center(
                child: TextButton(
                  onPressed: _submitting ? null : widget.onCancel,
                  child: const Text('不同意,退出', style: TextStyle(color: MedMe.faint)),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _ConsentPoint extends StatelessWidget {
  const _ConsentPoint({
    required this.icon,
    required this.title,
    required this.body,
  });

  final IconData icon;
  final String title;
  final String body;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 8),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(icon, color: MedMe.proxyOrange, size: 22),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  title,
                  style: const TextStyle(fontSize: 14.5, fontWeight: FontWeight.w700),
                ),
                const SizedBox(height: 2),
                Text(
                  body,
                  style: const TextStyle(
                    fontSize: 13.5,
                    color: MedMe.faint,
                    height: 1.45,
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}
