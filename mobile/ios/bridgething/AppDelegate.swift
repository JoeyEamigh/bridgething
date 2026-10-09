import BridgethingCompanion
import BridgethingCompanionCore
import ExternalAccessory
import React
import React_RCTAppDelegate
import ReactAppDependencyProvider
import UIKit
import os

@main
class AppDelegate: UIResponder, UIApplicationDelegate, RNAppAuthAuthorizationFlowManager {
  var window: UIWindow?

  public weak var authorizationFlowManagerDelegate: RNAppAuthAuthorizationFlowManagerDelegate?

  var reactNativeDelegate: ReactNativeDelegate?
  var reactNativeFactory: RCTReactNativeFactory?

  private var heldLaunchOptions: [UIApplication.LaunchOptionsKey: Any]?
  private var unlockObservers: [NSObjectProtocol] = []

  func application(
    _: UIApplication,
    didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
  ) -> Bool {
    bootstrapLogging()
    CrashCapture.install()
    if ProtectedStorage.isReadable() {
      start(launchOptions: launchOptions)
    } else {
      holdUntilFirstUnlock(launchOptions: launchOptions)
    }
    return true
  }

  func application(
    _: UIApplication,
    open url: URL,
    options _: [UIApplication.OpenURLOptionsKey: Any] = [:]
  ) -> Bool {
    authorizationFlowManagerDelegate?.resumeExternalUserAgentFlow(with: url) ?? false
  }

  private func holdUntilFirstUnlock(launchOptions: [UIApplication.LaunchOptionsKey: Any]?) {
    Logger(subsystem: "com.bridgething", category: "launch")
      .notice("storage is locked until the first unlock after boot, holding startup")
    heldLaunchOptions = launchOptions
    let center = NotificationCenter.default
    let wakes: [Notification.Name] = [
      UIApplication.protectedDataDidBecomeAvailableNotification,
      UIApplication.didBecomeActiveNotification,
      .EAAccessoryDidConnect,
    ]
    unlockObservers = wakes.map { name in
      center.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
        MainActor.assumeIsolated { self?.startIfUnlocked() }
      }
    }
    EAAccessoryManager.shared().registerForLocalNotifications()
  }

  private func startIfUnlocked() {
    guard !unlockObservers.isEmpty, ProtectedStorage.isReadable() else { return }
    let center = NotificationCenter.default
    for observer in unlockObservers { center.removeObserver(observer) }
    unlockObservers = []
    EAAccessoryManager.shared().unregisterForLocalNotifications()
    let launchOptions = heldLaunchOptions
    heldLaunchOptions = nil
    start(launchOptions: launchOptions)
  }

  private func start(launchOptions: [UIApplication.LaunchOptionsKey: Any]?) {
    CompanionLogs.shared.install()
    persistReactNativeLogs()
    BridgethingApp.installBridgething()

    let delegate = ReactNativeDelegate()
    let factory = RCTReactNativeFactory(delegate: delegate)
    delegate.dependencyProvider = RCTAppDependencyProvider()

    reactNativeDelegate = delegate
    reactNativeFactory = factory

    window = UIWindow(frame: UIScreen.main.bounds)

    factory.startReactNative(
      withModuleName: "bridgething",
      in: window,
      launchOptions: launchOptions
    )
  }

  private func persistReactNativeLogs() {
    RCTAddLogFunction { level, source, _, _, message in
      guard let message else { return }
      let label = source == .javaScript ? "js" : "react-native"
      CompanionLogs.shared.store?.record(level: Self.storeLevel(level), label: label, message: message)
    }
  }

  private nonisolated static func storeLevel(_ level: RCTLogLevel) -> LogStoreLevel {
    switch level {
    case .trace: .trace
    case .info: .info
    case .warning: .warn
    case .error: .error
    case .fatal: .fatal
    @unknown default: .info
    }
  }
}

class ReactNativeDelegate: RCTDefaultReactNativeFactoryDelegate {
  override func sourceURL(for _: RCTBridge) -> URL? {
    bundleURL()
  }

  override func bundleURL() -> URL? {
    #if DEBUG
      RCTBundleURLProvider.sharedSettings().jsBundleURL(forBundleRoot: "index")
    #else
      Bundle.main.url(forResource: "main", withExtension: "jsbundle")
    #endif
  }
}
