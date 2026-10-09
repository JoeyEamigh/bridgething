import Foundation

public enum ProtectedStorage {
  public static func isReadable() -> Bool {
    #if os(iOS)
      let files = FileManager.default
      guard let base = files.urls(for: .applicationSupportDirectory, in: .userDomainMask).first else { return true }
      do {
        try files.createDirectory(at: base, withIntermediateDirectories: true)
        try Data([1]).write(
          to: base.appendingPathComponent("first-unlock-probe"),
          options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication]
        )
        return true
      } catch {
        return false
      }
    #else
      return true
    #endif
  }
}
