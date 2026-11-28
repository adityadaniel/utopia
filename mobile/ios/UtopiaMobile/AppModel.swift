// The state one phone screen reads: today's note, the draft, the picker and the sync
// status. Every core call leaves the main actor (CoreClient), so a view never waits for the
// store or the relay (C5, AC-9.6).

import Foundation
import Observation

/// What the sync line in the header says (ios-prd IO-9). A wrong token is its own answer,
/// never "Offline" (AC-9.4).
enum SyncState: Equatable, Sendable {
    case off
    case syncing
    case synced
    case offline(queued: UInt32)
    case needsAuth
    case unsupported(version: UInt32)
    /// Pages that hold another device's changes which cannot apply yet, because that device
    /// has not sent this relay the earlier changes they build on. They are kept, not lost
    /// (§12.6, §2.3.7), so this is not "Synced" and not "Sync failed".
    case blocked(pages: UInt32)
    case failed(String)

    var text: String {
        switch self {
        case .off: "Not syncing"
        case .syncing: "Syncing…"
        case .synced: "Synced"
        case let .offline(queued):
            queued == 1 ? "Offline — 1 change queued" : "Offline — \(queued) changes queued"
        case .needsAuth: "Sign-in required"
        case let .unsupported(version):
            version > 0 ? "Update the app (relay v\(version))" : "Update the app"
        case let .blocked(pages):
            pages == 1
                ? "1 page waits for changes from another device"
                : "\(pages) pages wait for changes from another device"
        case let .failed(message): message
        }
    }

    /// The same answer in the two or three words the header has room for (§4.3: "a short
    /// text"). The full sentence is what VoiceOver and the settings button read.
    var shortText: String {
        switch self {
        case .off: "Not syncing"
        case .syncing: "Syncing…"
        case .synced: "Synced"
        case let .offline(queued): queued == 0 ? "Offline" : "Offline · \(queued)"
        case .needsAuth: "Sign in"
        case .unsupported: "Update app"
        case let .blocked(pages): "Waiting · \(pages)"
        case .failed: "Sync failed"
        }
    }
}

/// Where the caret should go after a change, in UTF-16 units of one draft line.
struct CaretRequest: Equatable, Sendable {
    var line: UUID
    var offset: Int
    var token = UUID()
}

@MainActor
@Observable
final class AppModel {
    enum Phase: Equatable {
        case launching
        case setup
        case locked
        case ready
    }

    enum Picker: Equatable {
        case link
        case date
    }

    private(set) var phase: Phase = .launching
    private(set) var today: PageView?
    private(set) var earlier: [PageView] = []
    private(set) var syncState: SyncState = .off
    private(set) var errorMessage: String?
    private(set) var sending = false
    private(set) var recentPages: [PageRef] = []
    /// The entry the log scrolls to after a Send (§4.4).
    private(set) var lastSentEntry: String?

    /// The draft lines, kept after each change (rule 7).
    var draft = DraftStore.load()
    var activeLine: UUID?
    var caretRequest: CaretRequest?
    var picker: Picker?

    /// The chips for the active line (§4.6), and the line they belong to.
    private(set) var suggestions: [TitleMatch] = []
    private(set) var suggestionLine: UUID?
    /// The drawn pieces of each draft line: plain runs and accepted links (C1, AC-4.1).
    private(set) var lineParts: [UUID: [TextPart]] = [:]

    private var core: CoreClient?
    private var settings = SyncSettings.load()
    private var syncTask: Task<Void, Never>?
    private var suggestionTask: Task<Void, Never>?
    #if DEBUG
    /// The mock answers of a SwiftUI preview. A model that has one never opens a graph.
    private var fixture: PreviewFixture?
    #endif

    var isConfigured: Bool { core != nil }
    var canSync: Bool { settings.isConfigured }

    var activeIndex: Int? {
        guard let activeLine else { return nil }
        return draft.index(of: activeLine)
    }

    // MARK: - Launch

    func launch() async {
        #if DEBUG
        if fixture != nil { return }
        #endif
        errorMessage = nil
        let exists = FileManager.default.fileExists(atPath: Paths.graph.path)
        guard exists else {
            // A phone with no graph asks first, and writes nothing (AC-2.4, AC-10.1).
            phase = .setup
            return
        }
        do {
            try await openGraph(passphrase: Keychain.get(Keychain.databasePassphrase))
        } catch FfiError.Locked {
            phase = .locked
        } catch {
            errorMessage = Self.describe(error)
            phase = .setup
        }
    }

    /// "Start a new graph" (AC-10.1): the file is made, the Today screen opens.
    func startNewGraph() async {
        do {
            try await openGraph(passphrase: nil)
        } catch {
            errorMessage = Self.describe(error)
        }
    }

    /// "Connect to my graph" (AC-10.2): the relay settings and the secrets are kept, one sync
    /// brings the Mac's pages, then Today opens.
    func connect(relayURL: String, graphID: String, token: String, passphrase: String, databasePassphrase: String?) async {
        settings = SyncSettings(relayURL: relayURL.trimmingCharacters(in: .whitespaces), graphID: graphID.trimmingCharacters(in: .whitespaces))
        settings.save()
        Keychain.set(Keychain.relayToken, token)
        Keychain.set(Keychain.e2ePassphrase, passphrase)
        if let databasePassphrase, !databasePassphrase.isEmpty {
            Keychain.set(Keychain.databasePassphrase, databasePassphrase)
        }
        await startNewGraph()
        guard core != nil else { return }
        await syncNow()
    }

    func unlock(passphrase: String) async {
        do {
            Keychain.set(Keychain.databasePassphrase, passphrase)
            try await openGraph(passphrase: passphrase)
        } catch {
            errorMessage = "That passphrase does not open this graph."
        }
    }

    private func openGraph(passphrase: String?) async throws {
        let client = try await CoreClient.open(path: Paths.graph.path, passphrase: passphrase)
        core = client
        phase = .ready
        await refresh()
        await syncNow()
    }

    // MARK: - Reading

    func refresh() async {
        guard let core else { return }
        do {
            let day = try await core.today()
            today = day
            // The log shows today only (§4.4); the day before is kept for Find's "Yesterday"
            // row (AC-8.2).
            if let date = day.date {
                earlier = try await core.daysBefore(date, count: 1)
            }
            if draft.lines.isEmpty {
                draft = Draft()
            }
            if activeLine == nil || draft.index(of: activeLine!) == nil {
                // A draft that was not sent keeps its caret at the end of its last line
                // (AC-2.5), so the next thought continues it.
                if let last = draft.lines.last {
                    focusLine(last.id, at: last.text.utf16.count)
                }
            }
        } catch {
            errorMessage = Self.describe(error)
        }
    }

    func page(_ id: String) async -> PageView? {
        #if DEBUG
        if let fixture { return fixture.pages[id] }
        #endif
        return try? await core?.page(id)
    }

    func backlinks(_ id: String) async -> [(pageTitle: String, hits: [RefView])] {
        let found: [RefView]?
        #if DEBUG
        if let fixture {
            found = fixture.backlinks[id] ?? []
        } else {
            found = try? await core?.backlinks(id)
        }
        #else
        found = try? await core?.backlinks(id)
        #endif
        guard let refs = found else { return [] }
        var order: [String] = []
        var grouped: [String: [RefView]] = [:]
        for ref in refs {
            if grouped[ref.pageId] == nil {
                order.append(ref.pageId)
            }
            grouped[ref.pageId, default: []].append(ref)
        }
        return order.map { (grouped[$0]?.first?.pageTitle ?? "", grouped[$0] ?? []) }
    }

    func search(_ words: String) async -> SearchResults? {
        #if DEBUG
        if let fixture { return fixture.search(words) }
        #endif
        return try? await core?.search(words)
    }

    func recent() async -> [PageRef] {
        #if DEBUG
        if let fixture { return fixture.recent }
        #endif
        return (try? await core?.recentPages()) ?? []
    }

    func refreshRecents() async {
        recentPages = await recent()
    }

    // MARK: - The draft

    func focusLine(_ id: UUID?, at offset: Int?) {
        activeLine = id
        if let id, let offset {
            caretRequest = CaretRequest(line: id, offset: offset)
        }
        refreshSuggestions()
    }

    func setText(_ text: String, for id: UUID) {
        guard let index = draft.index(of: id) else { return }
        draft.lines[index].text = text
        draft.lines[index].dropStaleLinks()
        draft.applyDepthRule()
        DraftStore.save(draft)
        refreshParts(for: draft.lines[index])
        refreshSuggestions()
    }

    /// Rule 3: Return makes a new line below and puts the caret in it.
    func newLine(after id: UUID) -> UUID? {
        guard let index = draft.index(of: id) else { return nil }
        let newIndex = draft.newLine(after: index)
        DraftStore.save(draft)
        let line = draft.lines[newIndex]
        focusLine(line.id, at: 0)
        return line.id
    }

    /// A paste of several lines (§7): each line becomes a line of the draft, and leading tabs
    /// or two-space steps become its depth. The depth rule then applies as after any change.
    func pasteLines(_ texts: [String], after id: UUID) {
        guard let index = draft.index(of: id) else { return }
        let base = draft.lines[index]
        var inserted: [DraftLine] = []
        for (offset, raw) in texts.enumerated() {
            let line = raw.replacingOccurrences(of: "\t", with: "  ")
            let spaces = line.prefix { $0 == " " }.count
            let text = String(line.dropFirst(spaces))
            var depth = base.depth + spaces / 2
            // The first pasted line continues the active line, so it keeps its depth.
            if offset == 0 {
                depth = base.depth
            }
            inserted.append(DraftLine(text: text, depth: depth, todo: base.todo))
        }
        guard !inserted.isEmpty else { return }
        if base.isEmpty {
            draft.lines.remove(at: index)
            draft.lines.insert(contentsOf: inserted, at: index)
        } else {
            draft.lines.insert(contentsOf: inserted, at: index + 1)
        }
        draft.applyDepthRule()
        DraftStore.save(draft)
        let last = inserted.last
        if let last, let at = draft.index(of: last.id) {
            draft.lines[at].dropStaleLinks()
            focusLine(last.id, at: draft.lines[at].text.utf16.count)
        }
    }

    /// Rule 4: delete at the start of an empty line.
    func deleteEmptyLine(_ id: UUID) {
        guard let index = draft.index(of: id) else { return }
        guard let target = draft.deleteEmptyLine(at: index) else { return }
        DraftStore.save(draft)
        focusLine(target.id, at: target.caretAtEnd ? endOf(target.id) : nil)
    }

    func deeper() {
        guard let index = activeIndex else { return }
        draft.deeper(index)
        persistAndPaint()
    }

    func shallower() {
        guard let index = activeIndex else { return }
        draft.shallower(index)
        persistAndPaint()
    }

    func toggleTodo() {
        guard let index = activeIndex else { return }
        draft.lines[index].todo.toggle()
        persistAndPaint()
    }

    var canGrowActive: Bool { activeIndex.map(draft.canGrow) ?? false }
    var canShrinkActive: Bool { activeIndex.map(draft.canShrink) ?? false }
    var activeIsTodo: Bool { activeIndex.map { draft.lines[$0].todo } ?? false }
    var canSend: Bool { !draft.isEmptyText && !sending }

    private func persistAndPaint() {
        DraftStore.save(draft)
        refreshSuggestions()
        if let index = activeIndex {
            refreshParts(for: draft.lines[index])
        }
    }

    private func endOf(_ id: UUID) -> Int? {
        guard let index = draft.index(of: id) else { return nil }
        return draft.lines[index].text.utf16.count
    }

    // MARK: - Suggestions (§4.6)

    private func refreshSuggestions() {
        suggestionTask?.cancel()
        guard let index = activeIndex else {
            suggestions = []
            suggestionLine = nil
            return
        }
        let line = draft.lines[index]
        suggestionLine = line.id
        guard let core, !line.text.isEmpty else {
            suggestions = []
            return
        }
        let links = line.links.map(\.accepted)
        suggestionTask = Task { [weak self] in
            let found = (try? await core.titleSuggestions(line.text, accepted: links)) ?? []
            guard !Task.isCancelled, let self, self.activeLine == line.id else { return }
            self.suggestions = found
        }
    }

    /// The words stay as typed and show in link colour; only the range is recorded (§4.6).
    func acceptSuggestion(_ match: TitleMatch) {
        guard let index = activeIndex else { return }
        draft.lines[index].links.append(
            DraftLink(title: match.title, start: Int(match.start), end: Int(match.end))
        )
        DraftStore.save(draft)
        refreshParts(for: draft.lines[index])
        refreshSuggestions()
    }

    // MARK: - The page picker (§4.6)

    func showPicker(_ which: Picker) {
        picker = which
        if which == .link {
            Task { recentPages = await recent() }
        }
    }

    func closePicker() { picker = nil }

    /// Insert a page title at the caret of the active line as an accepted link, with a space
    /// after it (AC-4.5, AC-4.6). No page is made here: the Send makes it (AC-4.6).
    func insertLink(title: String) {
        guard let index = activeIndex else { return }
        let line = draft.lines[index]
        let caret = min(caretRequest?.line == line.id ? caretRequest?.offset ?? line.text.utf16.count : line.text.utf16.count, line.text.utf16.count)
        let text = line.text
        let at = text.utf16.index(text.utf16.startIndex, offsetBy: caret)
        var updated = text
        updated.insert(contentsOf: title + " ", at: at)
        draft.lines[index].text = updated
        draft.lines[index].links.append(
            DraftLink(title: title, start: caret, end: caret + title.utf16.count)
        )
        DraftStore.save(draft)
        refreshParts(for: draft.lines[index])
        picker = nil
        focusLine(line.id, at: caret + title.utf16.count + 1)
    }

    /// The title of a day as this graph writes it, for the Date control (AC-4.9).
    func title(forISODate iso: String) async -> String {
        if let day = try? await core?.day(iso), !day.title.isEmpty {
            return day.title
        }
        return iso
    }

    func sqlDate(_ date: Date) -> String {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = .current
        let parts = calendar.dateComponents([.year, .month, .day], from: date)
        return String(format: "%04d-%02d-%02d", parts.year ?? 1970, parts.month ?? 1, parts.day ?? 1)
    }

    // MARK: - Send (§4.5 rule 5)

    func send() async {
        guard let core, canSend else { return }
        sending = true
        defer { sending = false }
        // Each line's links are written as `[[Title]]` by the core (C1, AC-4.1).
        var stored: [UUID: String] = [:]
        do {
            for line in draft.lines where !line.isEmpty {
                stored[line.id] = try await core.textWithLinks(line.text, links: line.links.map(\.accepted))
            }
            let outgoing = draft.outgoing { stored[$0.id] ?? $0.text }
            guard !outgoing.isEmpty else { return }
            let appended = try await core.appendOutline(outgoing)
            today = appended
            lastSentEntry = appended.entries.last?.id
            draft = Draft()
            DraftStore.save(draft)
            lineParts = [:]
            suggestions = []
            focusLine(draft.lines.first?.id, at: 0)
            await refresh()
        } catch {
            errorMessage = Self.describe(error)
            return
        }
        await syncNow()
    }

    // MARK: - A to-do (§4.4, K9)

    func toggleTodo(block: BlockView) async {
        #if DEBUG
        if fixture != nil {
            paintTodo(block.id, done: !block.checked)
            return
        }
        #endif
        guard let core else { return }
        let done = !block.checked
        // The tick shows at once and does not wait for sync (AC-5.3).
        paintTodo(block.id, done: done)
        do {
            try await core.setTodoDone(block.id, done: done)
        } catch {
            paintTodo(block.id, done: block.checked)
            errorMessage = Self.describe(error)
            return
        }
        today = try? await core.today()
        await syncNow()
    }

    private func paintTodo(_ blockID: String, done: Bool) {
        guard var day = today else { return }
        for i in day.blocks.indices where day.blocks[i].id == blockID {
            day.blocks[i].checked = done
        }
        today = day
    }

    // MARK: - Sync (IO-9)

    func syncNow() async {
        guard let core, canSync, syncTask == nil else { return }
        syncState = .syncing
        syncTask = Task { [weak self] in
            defer { self?.syncTask = nil }
            let outcome: SyncOutcome?
            do {
                outcome = try await core.sync(self?.settings ?? SyncSettings())
            } catch {
                self?.syncState = .failed(Self.describe(error))
                return
            }
            guard let outcome else { return }
            guard let self else { return }
            switch outcome {
            case .synced:
                self.syncState = .synced
                // A remote copy replaces the draft for the day, so read it again.
                self.today = try? await core.today()
            case let .partial(summary, _, blocked):
                // A round that applied most pages still changed today's note, but it is
                // not "Synced": a failed page or a page that waits must show (§12.6).
                self.syncState = Self.partialState(failed: summary.failed, blocked: blocked)
                self.today = try? await core.today()
            case let .offline(queued):
                self.syncState = .offline(queued: queued)
            case .needsAuth:
                self.syncState = .needsAuth
            case let .unsupported(version):
                self.syncState = .unsupported(version: version)
            case .busy:
                break
            case let .localError(message):
                self.syncState = .failed(message)
            }
        }
    }

    func setSync(relayURL: String, graphID: String, token: String, passphrase: String) {
        settings = SyncSettings(relayURL: relayURL, graphID: graphID)
        settings.save()
        Keychain.set(Keychain.relayToken, token)
        Keychain.set(Keychain.e2ePassphrase, passphrase)
    }

    // MARK: - Parts for the draft

    private func refreshParts(for line: DraftLine) {
        guard let core else { return }
        let text = line.text
        let links = line.links.map(\.accepted)
        let id = line.id
        Task { [weak self] in
            let parts = (try? await core.draftParts(text, links: links)) ?? []
            guard let self, self.draft.index(of: id).map({ self.draft.lines[$0].text }) == text else { return }
            self.lineParts[id] = parts
        }
    }

    // MARK: - Errors

    /// The header state for a round that applied but left pages failed or waiting.
    static func partialState(failed: UInt32, blocked: UInt32) -> SyncState {
        if failed > 0 {
            return .failed(failed == 1 ? "1 page could not sync" : "\(failed) pages could not sync")
        }
        return blocked > 0 ? .blocked(pages: blocked) : .synced
    }

    static func describe(_ error: any Error) -> String {
        if let ffi = error as? FfiError {
            switch ffi {
            case .Locked: return "This graph is encrypted. Its passphrase is missing or wrong."
            case let .NotFound(message): return "Not found: \(message)"
            case let .Invalid(message): return message
            case let .Sync(message): return "Sync: \(message)"
            case let .Internal(message): return message
            }
        }
        return error.localizedDescription
    }
}

#if DEBUG
/// The mock answers a preview model gives in place of the core (`PreviewData`).
struct PreviewFixture {
    var pages: [String: PageView] = [:]
    var backlinks: [String: [RefView]] = [:]
    var recent: [PageRef] = []

    /// A title search over the mock pages, and a word search over their blocks.
    func search(_ words: String) -> SearchResults {
        let needle = words.lowercased()
        let found = pages.values
            .filter { $0.title.lowercased().contains(needle) }
            .sorted { $0.title < $1.title }
            .map { PageRef(id: $0.id, title: $0.title, date: $0.date) }
        let hits = pages.values.sorted { $0.title < $1.title }.flatMap { page in
            page.blocks
                .filter { $0.plain.lowercased().contains(needle) }
                .map { SearchHit(pageId: page.id, pageTitle: page.title, blockId: $0.id, text: $0.plain) }
        }
        return SearchResults(pages: found, blocks: hits)
    }
}

extension AppModel {
    /// A model with no graph, filled with mock data, for SwiftUI previews. It never opens a
    /// graph and never calls the relay.
    static func preview(
        phase: Phase = .ready,
        today: PageView? = PreviewData.today,
        earlier: [PageView] = [PreviewData.yesterday],
        syncState: SyncState = .synced,
        draft: Draft = PreviewData.draft,
        suggestions: [TitleMatch] = [],
        picker: Picker? = nil,
        errorMessage: String? = nil,
        fixture: PreviewFixture = PreviewData.fixture
    ) -> AppModel {
        let model = AppModel()
        model.fixture = fixture
        model.phase = phase
        model.today = today
        model.earlier = earlier
        model.syncState = syncState
        model.draft = draft
        model.activeLine = draft.lines.last?.id
        model.suggestions = suggestions
        model.suggestionLine = suggestions.isEmpty ? nil : draft.lines.last?.id
        model.picker = picker
        model.errorMessage = errorMessage
        model.recentPages = fixture.recent
        return model
    }
}
#endif
