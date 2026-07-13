import Flutter
import UIKit

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
}
