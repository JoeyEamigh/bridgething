import BridgethingCompanionCore
import BridgethingCrashHooks
import Foundation
#if canImport(os)
  import os
#endif
#if os(iOS)
  import MetricKit
#endif

public enum CrashCapture {
  public static func install() {
    bridgething_install_crash_hooks { message in
      guard let message else { return }
      CrashCapture.record(.fatal, label: "crash", String(cString: message))
      CompanionLogs.shared.store?.flush()
    }
    #if os(iOS)
      MXMetricManager.shared.add(CrashDiagnostics.shared)
    #endif
  }

  static func record(_ level: LogStoreLevel, label: String, _ text: String) {
    #if canImport(os)
      let log = os.Logger(subsystem: "com.bridgething", category: label)
      if level == .fatal {
        log.fault("\(text, privacy: .public)")
      } else {
        log.error("\(text, privacy: .public)")
      }
    #endif
    CompanionLogs.shared.store?.record(level: level, label: label, message: text)
  }
}

#if os(iOS)
  final class CrashDiagnostics: NSObject, MXMetricManagerSubscriber, @unchecked Sendable {
    static let shared = CrashDiagnostics()

    func didReceive(_ payloads: [MXDiagnosticPayload]) {
      for payload in payloads {
        for crash in payload.crashDiagnostics ?? [] {
          CrashCapture.record(.error, label: "metrickit", Self.describe(crash))
        }
      }
    }

    static func describe(_ crash: MXCrashDiagnostic) -> String {
      var parts = [
        "earlier crash reported by metrickit",
        "build \(crash.metaData.applicationBuildVersion)",
        "os \(crash.metaData.osVersion)",
      ]
      if let type = crash.exceptionType { parts.append("exception type \(type)") }
      if let code = crash.exceptionCode { parts.append("exception code \(code)") }
      if let signal = crash.signal { parts.append("signal \(signal)") }
      if let reason = crash.terminationReason { parts.append("termination \(reason)") }
      if let objc = crash.exceptionReason { parts.append("\(objc.exceptionName): \(objc.composedMessage)") }
      let stack = String(decoding: crash.callStackTree.jsonRepresentation(), as: UTF8.self)
      return parts.joined(separator: " | ") + "\n" + stack
    }
  }
#endif
