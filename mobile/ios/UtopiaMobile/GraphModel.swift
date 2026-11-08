// The app's view of the Rust core (PRD §14): one graph, opened from the app's
// documents folder, used from the main actor. Every call is local and fast; sync is the
// only network operation and runs off the main thread.

import Foundation
import SwiftUI

@MainActor
final class GraphModel: ObservableObject {
    @Published private(set) var today: PageView?
    @Published var error: String?
    @Published private(set) var syncing = false
    @Published private(set) var lastSync: String?

    private(set) var graph: Graph?

    /// Where the graph lives: `UTOPIA_MOBILE_GRAPH`, else Documents/graph.utopia.
    static func graphURL() -> URL {
        if let p = ProcessInfo.processInfo.environment["UTOPIA_MOBILE_GRAPH"] {
            return URL(fileURLWithPath: p)
        }
        let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        return docs.appendingPathComponent("graph.utopia")
    }

    init(graph: Graph? = nil) {
        do {
            let g = try graph ?? Graph.open(path: Self.graphURL().path, passphrase: Keychain.get("utopia-db"))
            if let d = ProcessInfo.processInfo.environment["UTOPIA_TODAY"] {
                try g.setToday(date: d)
            }
            self.graph = g
            reload()
        } catch {
            self.error = "\(error)"
        }
    }

    func reload() {
        guard let graph else { return }
        do {
            today = try graph.today()
        } catch {
            self.error = "\(error)"
        }
    }

    /// Add a line to today's page (FR-1.2): the core fills the empty first block.
    func capture(_ text: String) {
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let graph, let day = today, !t.isEmpty else { return }
        do {
            _ = try graph.appendBlock(pageId: day.id, text: t)
            reload()
        } catch {
            self.error = "\(error)"
        }
    }

    func edit(_ block: BlockView, to text: String) {
        guard let graph else { return }
        do {
            try graph.setBlockText(blockId: block.id, text: text)
            reload()
        } catch {
            self.error = "\(error)"
        }
    }

    func page(id: String) -> PageView? {
        try? graph?.page(pageId: id)
    }

    func page(titled title: String) -> PageView? {
        (try? graph?.findPage(title: title)) ?? nil
    }

    func backlinks(_ pageId: String) -> [RefView] {
        (try? graph?.backlinks(pageId: pageId)) ?? []
    }

    func suggestions(for query: String) -> [String] {
        guard !query.isEmpty else { return [] }
        return (try? graph?.searchTitles(query: query, limit: 6)) ?? []
    }

    /// One sync round with the configured relay, off the main thread.
    ///
    /// A tap while a round is running starts nothing (the core would answer `busy`), and
    /// the round's outcome says *why* it did not finish: a wrong token or another protocol
    /// version is not "offline" (§12.6).
    func sync(settings: SyncSettings) {
        guard let graph, !settings.relayURL.isEmpty, !syncing else { return }
        syncing = true
        let token = Keychain.get("utopia-sync")
        let passphrase = Keychain.get("utopia-sync-passphrase")
        Task.detached {
            let result = Result {
                try graph.sync(
                    relayUrl: settings.relayURL, graphId: settings.graphID,
                    token: token, e2ePassphrase: passphrase)
            }
            await MainActor.run {
                self.syncing = false
                switch result {
                case .success(let outcome):
                    self.lastSync = Self.describe(outcome)
                    self.reload()
                case .failure(let e):
                    // Only a transport that cannot be built (a bad relay URL) gets here.
                    self.lastSync = "Sync: \(e)"
                }
            }
        }
    }

    private static func describe(_ outcome: SyncOutcome) -> String {
        switch outcome {
        case .synced(let s):
            return "Synced: \(s.pushed) sent, \(s.importedPages) pages updated"
        case .partial(let s, let queued):
            let failed = s.failed > 0 ? " — \(s.failed) page(s) failed" : ""
            let queue = queued > 0 ? " — \(queued) changes queued" : ""
            return "Synced: \(s.pushed) sent, \(s.importedPages) pages updated\(failed)\(queue)"
        case .offline(let queued):
            return "Offline — \(queued) changes queued"
        case .localError(let message):
            return "Sync: local error — \(message)"
        case .needsAuth:
            return "Sync: sign-in required"
        case .unsupported(let version):
            let which = version > 0 ? "protocol \(version)" : "a newer protocol"
            return "Sync: relay speaks \(which); update the app"
        case .busy:
            return "Sync: already running"
        }
    }
}

struct SyncSettings: Codable, Equatable {
    var relayURL = ""
    var graphID = "graph"

    static func load() -> SyncSettings {
        guard let data = UserDefaults.standard.data(forKey: "sync"),
              let s = try? JSONDecoder().decode(SyncSettings.self, from: data)
        else { return SyncSettings() }
        return s
    }

    func save() {
        UserDefaults.standard.set(try? JSONEncoder().encode(self), forKey: "sync")
    }
}

/// Secrets (relay token, E2E passphrase, database passphrase) live in the Keychain,
/// never in the graph (§15.6).
enum Keychain {
    static func get(_ service: String) -> String? {
        let q: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var out: AnyObject?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess,
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
