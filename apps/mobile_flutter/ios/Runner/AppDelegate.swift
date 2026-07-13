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
  }
}

// MARK: - iCloud ubiquity-container bridge (iOS only)
//
// 保险箱「真相」(objects/ + log/)可存进本 App 的 iCloud ubiquity 容器,在用户的
// 苹果设备间同步;medme.db 留在沙盒本地、从日志重建。这里用 `@_cdecl` 暴露 Rust 需要
// 的两个系统调用为纯 C-ABI 符号——Runner target 编译本文件、链接 Rust 静态库,Rust
// 的 `extern "C"`(见 rust/src/icloud.rs)在同一二进制里直接解析到这些符号,无需插件。
// 从旧 Tauri 版 `apps/mobile/.../ICloud.swift` 逐字移植(机制完全一致)。

/// 解析本 App 的 iCloud ubiquity 容器,返回其 POSIX 路径(malloc 的 C 字符串,调用方
/// 用 `medme_icloud_free` 释放)。iCloud 不可用/未登录/容器无法配置时返回 null。
/// `url(forUbiquityContainerIdentifier:)` 是阻塞 I/O,绝不能在主线程跑,故切后台队列
/// 用信号量等结果(调用方是同步 FFI)。传 nil 选取第一个 ubiquity-container 授权值。
@_cdecl("medme_icloud_container_path")
public func medme_icloud_container_path() -> UnsafeMutablePointer<CChar>? {
  var resolved: String?
  let sem = DispatchSemaphore(value: 0)
  DispatchQueue.global(qos: .userInitiated).async {
    resolved = FileManager.default.url(forUbiquityContainerIdentifier: nil)?.path
    sem.signal()
  }
  sem.wait()
  guard let path = resolved, !path.isEmpty else { return nil }
  return path.withCString { strdup($0) }
}

/// 尽力触发一个被逐出(dataless)的 iCloud 项目下载,好让随后的读取能看到字节。
/// 返回是否成功发起下载请求(下载本身是异步的)。
@_cdecl("medme_icloud_ensure_downloaded")
public func medme_icloud_ensure_downloaded(_ pathC: UnsafePointer<CChar>) -> Bool {
  let url = URL(fileURLWithPath: String(cString: pathC))
  do {
    try FileManager.default.startDownloadingUbiquitousItem(at: url)
    return true
  } catch {
    return false
  }
}

/// 释放 `medme_icloud_container_path` 返回的缓冲区(strdup 用 malloc 分配)。
@_cdecl("medme_icloud_free")
public func medme_icloud_free(_ ptr: UnsafeMutablePointer<CChar>?) {
  if let ptr = ptr { free(ptr) }
}
