// The core, called off the main thread. Every FFI call blocks on the store lock, so each one
// runs in its own task and no view ever waits for it (C5, AC-9.6, §14.3).

import Foundation

struct CoreClient: Sendable {
    let graph: Graph

    private func run<T: Sendable>(_ body: @escaping @Sendable () throws -> T) async throws -> T {
        try await Task.detached(priority: .userInitiated) { try body() }.value
    }

    static func open(path: String, passphrase: String?) async throws -> CoreClient {
        let client = try await Task.detached(priority: .userInitiated) {
            try Graph.open(path: path, passphrase: passphrase)
        }.value
        if let today = ProcessInfo.processInfo.environment["UTOPIA_TODAY"] {
            _ = try? client.setToday(date: today)
        }
        return CoreClient(graph: client)
    }

    func setToday(_ date: String) async throws {
        try await run { try graph.setToday(date: date) }
    }

    func today() async throws -> PageView {
        try await run { try graph.today() }
    }

    func day(_ date: String) async throws -> PageView {
        try await run { try graph.day(date: date) }
    }

    func daysBefore(_ date: String, count: UInt32) async throws -> [PageView] {
        try await run { try graph.daysBefore(date: date, count: count) }
    }

    func page(_ id: String) async throws -> PageView {
        try await run { try graph.page(pageId: id) }
    }

    func appendOutline(_ lines: [OutlineLine]) async throws -> PageView {
        try await run { try graph.appendOutline(lines: lines) }
    }

    func setTodoDone(_ blockID: String, done: Bool) async throws {
        try await run { try graph.setTodoDone(blockId: blockID, done: done) }
    }

    func titleSuggestions(_ text: String, accepted: [AcceptedLink], limit: UInt32 = 6) async throws -> [TitleMatch] {
        try await run { try graph.titleSuggestions(text: text, accepted: accepted, limit: limit) }
    }

    func dateSuggestions(_ text: String, accepted: [AcceptedLink], limit: UInt32 = 6) async throws -> [DateMatch] {
        try await run { try graph.dateSuggestions(text: text, accepted: accepted, limit: limit) }
    }

    func textWithLinks(_ text: String, links: [AcceptedLink]) async throws -> String {
        try await run { try graph.textWithLinks(text: text, links: links) }
    }

    func draftParts(_ text: String, links: [AcceptedLink]) async throws -> [TextPart] {
        try await run { try graph.draftParts(text: text, links: links) }
    }

    func absorbTypedLinks(
        _ text: String,
        links: [AcceptedLink],
        caret: UInt32?
    ) async throws -> AbsorbedDraft {
        try await run { try graph.absorbTypedLinks(text: text, links: links, caret: caret) }
    }

    func search(_ words: String, limit: UInt32 = 40) async throws -> SearchResults {
        try await run { try graph.search(words: words, limit: limit) }
    }

    func recentPages(_ limit: UInt32 = 8) async throws -> [PageRef] {
        try await run { try graph.recentPages(limit: limit) }
    }

    func queuedChanges() async throws -> UInt32 {
        try await run { try graph.queuedChanges() }
    }

    func backlinks(_ pageID: String) async throws -> [RefView] {
        try await run { try graph.backlinks(pageId: pageID) }
    }

    func findPage(_ title: String) async throws -> PageView? {
        try await run { try graph.findPage(title: title) }
    }

    func sync(_ settings: SyncSettings) async throws -> SyncOutcome {
        let token = Keychain.get(Keychain.relayToken)
        let passphrase = Keychain.get(Keychain.e2ePassphrase)
        let url = settings.relayURL
        let graphID = settings.graphID
        return try await run {
            try graph.sync(relayUrl: url, graphId: graphID, token: token, e2ePassphrase: passphrase)
        }
    }
}

/// A link the reader accepted, in the form the core takes.
extension DraftLink {
    init(_ link: AcceptedLink) {
        self.init(
            title: link.title,
            start: Int(link.start),
            end: Int(link.end),
            day: link.day,
            words: link.words,
            typed: link.typed
        )
    }

    var accepted: AcceptedLink {
        AcceptedLink(
            title: title,
            start: UInt32(max(0, start)),
            end: UInt32(max(0, end)),
            day: day,
            words: words,
            typed: typed ?? (day == nil && words != nil)
        )
    }
}
