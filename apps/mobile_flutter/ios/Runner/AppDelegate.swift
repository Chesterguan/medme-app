import Flutter
import UIKit
import Vision

@main
@objc class AppDelegate: FlutterAppDelegate, FlutterImplicitEngineDelegate {
  override func application(
    _ application: UIApplication,
    didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
  ) -> Bool {
    return super.application(application, didFinishLaunchingWithOptions: launchOptions)
  }

  func didInitializeImplicitFlutterEngine(_ engineBridge: FlutterImplicitEngineBridge) {
    GeneratedPluginRegistrant.register(with: engineBridge.pluginRegistry)
    registerICloudChannel(engineBridge.pluginRegistry)
    registerOcrChannel(engineBridge.pluginRegistry)
  }

  // MARK: - iCloud MethodChannel(iOS-only)
  //
  // Dart 侧经此 channel 解析 iCloud ubiquity 容器路径,再作参数传给 Rust FFI
  // (open_vault / enable_icloud_sync)。放在 app target 里当 MethodChannel handler,
  // 不像之前的 @_cdecl C-ABI 那样要被 Rust 独立框架反向链接(Flutter 插件框架不允许,
  // 会 archive linker 失败)。保险箱真相(objects/+log/)存进容器同步,medme.db 留本地。
  private func registerICloudChannel(_ registry: FlutterPluginRegistry) {
    guard let registrar = registry.registrar(forPlugin: "MedMeICloud") else { return }
    let channel = FlutterMethodChannel(
      name: "medme/icloud", binaryMessenger: registrar.messenger())
    channel.setMethodCallHandler { call, result in
      switch call.method {
      case "containerPath":
        // url(forUbiquityContainerIdentifier:) 是阻塞 I/O,切后台线程;传 nil 取第一个
        // ubiquity-container 授权值。返回容器根路径,不可用/未登录返回 nil。
        DispatchQueue.global(qos: .userInitiated).async {
          let path = FileManager.default.url(forUbiquityContainerIdentifier: nil)?.path
          DispatchQueue.main.async { result(path) }
        }
      default:
        result(FlutterMethodNotImplemented)
      }
    }
  }

  // MARK: - OCR MethodChannel(iOS-only,Apple Vision)
  //
  // Dart 的 `ocr_bridge.dart` 在 iOS 走此 channel 调原生 Vision(中文识别更强、
  // 原生支持 HEIC),安卓侧走 ML Kit——功能一致、各用平台最强引擎。`recognize`
  // 收一个图片文件路径,返回 { text, confidence(0~1) }。识别失败降级为空文本
  // (上层据此走「仅存原件」),不抛。
  private func registerOcrChannel(_ registry: FlutterPluginRegistry) {
    guard let registrar = registry.registrar(forPlugin: "MedMeOCR") else { return }
    let channel = FlutterMethodChannel(
      name: "medme/ocr", binaryMessenger: registrar.messenger())
    channel.setMethodCallHandler { call, result in
      switch call.method {
      case "recognize":
        guard let args = call.arguments as? [String: Any],
          let path = args["path"] as? String
        else {
          result(FlutterError(code: "bad_args", message: "missing path", details: nil))
          return
        }
        // Vision 是 CPU/GPU 密集,切后台线程,别卡 UI。
        DispatchQueue.global(qos: .userInitiated).async {
          let out = AppDelegate.recognizeText(atPath: path)
          DispatchQueue.main.async { result(out) }
        }
      default:
        result(FlutterMethodNotImplemented)
      }
    }
  }

  /// 用 Apple Vision 识别一张图片(含 HEIC)的文字。返回 `{text, confidence}`;
  /// 打不开/识别不到时返回空文本 + `confidence = NSNull`(Dart 侧兜底)。
  private static func recognizeText(atPath path: String) -> [String: Any] {
    let empty: [String: Any] = ["text": "", "confidence": NSNull()]
    guard let image = UIImage(contentsOfFile: path), let cg = image.cgImage else {
      return empty
    }
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    request.usesLanguageCorrection = true
    // 简/繁中文 + 英文;顺序即优先级。zh-* 自 iOS 14 起支持,部署目标 15.5 满足。
    request.recognitionLanguages = ["zh-Hans", "zh-Hant", "en-US"]
    let handler = VNImageRequestHandler(
      cgImage: cg, orientation: cgOrientation(image.imageOrientation), options: [:])
    do {
      try handler.perform([request])
    } catch {
      return empty
    }
    guard let observations = request.results, !observations.isEmpty else {
      return empty
    }
    var lines: [String] = []
    var confs: [Float] = []
    for obs in observations {
      guard let top = obs.topCandidates(1).first else { continue }
      lines.append(top.string)
      confs.append(top.confidence)
    }
    let text = lines.joined(separator: "\n")
    let confidence: Any =
      confs.isEmpty ? NSNull() : Double(confs.reduce(0, +) / Float(confs.count))
    return ["text": text, "confidence": confidence]
  }

  /// UIImage 的 EXIF 方向 → Vision 需要的 CGImagePropertyOrientation
  /// (`cgImage` 丢方向;手机拍的照片常带旋转,不校正会识别错/识别不到)。
  private static func cgOrientation(_ o: UIImage.Orientation) -> CGImagePropertyOrientation {
    switch o {
    case .up: return .up
    case .down: return .down
    case .left: return .left
    case .right: return .right
    case .upMirrored: return .upMirrored
    case .downMirrored: return .downMirrored
    case .leftMirrored: return .leftMirrored
    case .rightMirrored: return .rightMirrored
    @unknown default: return .up
    }
  }
}
