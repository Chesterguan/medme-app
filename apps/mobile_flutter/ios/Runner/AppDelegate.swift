import Flutter
import ImageIO
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
      case "ensureDownloaded":
        // 保险箱 `objects/` 存进 iCloud 容器后,单个对象可能被 iCloud 逐出本地
        // (只剩 `.icloud` 占位符)。「查看原件」读盘前,Dart 传对象绝对路径来触发
        // 按需下载并等待其落地,避免打开被逐出的原件失败。见 Dart `IcloudBridge`。
        guard let args = call.arguments as? [String: Any],
          let path = args["path"] as? String
        else {
          result(FlutterError(code: "bad_args", message: "missing path", details: nil))
          return
        }
        DispatchQueue.global(qos: .userInitiated).async {
          let ok = AppDelegate.ensureUbiquitousItemDownloaded(atPath: path)
          DispatchQueue.main.async { result(ok) }
        }
      default:
        result(FlutterMethodNotImplemented)
      }
    }
  }

  /// 确保 iCloud 容器里的对象已下载到本地。被逐出成 `.icloud` 占位符时触发
  /// `startDownloadingUbiquitousItem` 并轮询等待 `.current`(有界超时)。
  ///
  /// 快路径:已是最新(`.current`)或该文件根本不是 iCloud 托管(如关了 iCloud、
  /// 本机沙盒库,拿不到 downloadingStatus)→ 立即返回 `true`,不触发任何下载。
  /// 触发下载失败或超时 → 返回 `false`,上层照常尝试读盘并在失败时优雅降级。
  private static func ensureUbiquitousItemDownloaded(atPath path: String) -> Bool {
    let url = URL(fileURLWithPath: path)
    let statusKey: URLResourceKey = .ubiquitousItemDownloadingStatusKey
    func downloadingStatus() -> URLUbiquitousItemDownloadingStatus? {
      return try? url.resourceValues(forKeys: [statusKey]).ubiquitousItemDownloadingStatus
    }
    guard let status = downloadingStatus() else {
      // 非 iCloud 托管文件(无此资源值)——已在本地,视作就绪。
      return true
    }
    if status == .current { return true }
    do {
      try FileManager.default.startDownloadingUbiquitousItem(at: url)
    } catch {
      return false
    }
    // 轮询等待下载完成,最多 ~30s;超时交由上层降级(原件仍安全保存)。
    let deadline = Date().addingTimeInterval(30)
    while Date() < deadline {
      if downloadingStatus() == .current { return true }
      Thread.sleep(forTimeInterval: 0.2)
    }
    return false
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
    // Layer-0 A:按阅读顺序排序(上→下、左→右),大纵向间隔处插空行分块 → 下游按段
    // 分块更准。Vision 不保证返回顺序;boundingBox 归一化、原点在左下,maxY 越大越靠上。
    let sorted = observations.sorted { a, b in
      if abs(a.boundingBox.maxY - b.boundingBox.maxY) > 0.01 {
        return a.boundingBox.maxY > b.boundingBox.maxY
      }
      return a.boundingBox.minX < b.boundingBox.minX
    }
    var lines: [String] = []
    var confs: [Float] = []
    var prevY: CGFloat? = nil
    for obs in sorted {
      guard let top = obs.topCandidates(1).first else { continue }
      if let py = prevY, py - obs.boundingBox.maxY > 0.03 {
        lines.append("") // 大纵向间隔 → 块边界(join 后成空行)
      }
      lines.append(top.string)
      confs.append(top.confidence)
      prevY = obs.boundingBox.maxY
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
