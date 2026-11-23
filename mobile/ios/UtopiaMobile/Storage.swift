// The local settings of the phone: where the graph lives, the relay it syncs with and the
// secrets that reach it. Secrets go to the Keychain and never into the graph (C6,
// ios-prd AC-10.4). The draft, recents and scroll position are local and never sync (C7).

import Foundation
import Security

enum Paths {
    /// `UTOPIA_MOBILE_GRAPH` names the graph for scripted runs; otherwise it lives in the
    /// app's Documents folder.
    static var graph: URL {
        if let p = ProcessInfo.processInfo.environment["UTOPIA_MOBILE_GRAPH"] {
            return URL(fileURLWithPath: p)
        }
        let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        return docs.appendingPathComponent("graph.utopia")
    }

    /// The folder that holds the draft: local to the phone, never synced (C7).
    static var support: URL {
        if let p = ProcessInfo.processInfo.environment["UTOPIA_MOBILE_CONFIG"] {
            return URL(fileURLWithPath: p)
        }
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return base.appendingPathComponent("Utopia", isDirectory: true)
    }

    static func ensureSupportFolder() {
        try? FileManager.default.createDirectory(at: support, withIntermediateDirectories: true)
    }
}

/// The relay settings of this phone (A3: one graph). Non-secret fields live in
/// `UserDefaults`; the token and the passphrase live only in the Keychain.
struct SyncSettings: Codable, Equatable, Sendable {
    var relayURL = ""
    var graphID = ""

    var isConfigured: Bool { !relayURL.isEmpty && !graphID.isEmpty }

    static func load() -> SyncSettings {
        guard let data = UserDefaults.standard.data(forKey: "utopia.sync"),
              let s = try? JSONDecoder().decode(SyncSettings.self, from: data)
        else { return SyncSettings() }
        return s
    }

    func save() {
        UserDefaults.standard.set(try? JSONEncoder().encode(self), forKey: "utopia.sync")
    }
}

/// The Keychain items of this app. Only this file names a service.
enum Keychain {
    static let relayToken = "utopia-sync"
    static let e2ePassphrase = "utopia-sync-passphrase"
    static let databasePassphrase = "utopia-db"

    static func get(_ service: String) -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var out: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &out) == errSecSuccess,
              let data = out as? Data
        else { return nil }
        return String(data: data, encoding: .utf8)
    }

    static func set(_ service: String, _ value: String?) {
        let base: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
        ]
        SecItemDelete(base as CFDictionary)
        guard let value, !value.isEmpty else { return }
        var add = base
        add[kSecValueData as String] = Data(value.utf8)
        SecItemAdd(add as CFDictionary, nil)
    }
}
