// The draft: the lines in the compose box that are not sent (ios-prd §4.2, §4.5). It lives
// only on this phone and never syncs (C7), and it is kept after each change, so it is still
// there after the app is closed, killed or the phone restarted (rule 7, AC-1.5).

import Foundation

/// A link the reader accepted in a draft line: the title, and the range of the words that
/// carry it. The range is measured in UTF-16 units, like the core's `AcceptedLink`.
struct DraftLink: Codable, Equatable, Sendable {
    var title: String
    var start: Int
    var end: Int
    var day: String? = nil
    var words: String? = nil
}

struct DraftLine: Identifiable, Codable, Equatable, Sendable {
    var id: UUID
    var text: String
    var depth: Int
    var todo: Bool
    var links: [DraftLink]

    init(id: UUID = UUID(), text: String = "", depth: Int = 0, todo: Bool = false, links: [DraftLink] = []) {
        self.id = id
        self.text = text
        self.depth = depth
        self.todo = todo
        self.links = links
    }

    var isEmpty: Bool { text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    /// The words of `text` that a link covers, when it still covers any: a link whose words
    /// the reader changed is dropped (AC-4.11). The core rechecks the title on Send.
    mutating func dropStaleLinks() {
        let utf16 = Array(text.utf16)
        links = links.filter { link in
            link.start >= 0 && link.end <= utf16.count && link.start < link.end
                && String(decoding: utf16[link.start..<link.end], as: UTF16.self) != ""
        }
    }
}

/// The whole draft and the rules that keep it valid (§4.5 rules 1–8).
struct Draft: Equatable, Sendable {
    var lines: [DraftLine]

    static let maxDepth = 11

    init(lines: [DraftLine] = [DraftLine()]) {
        self.lines = lines.isEmpty ? [DraftLine()] : lines
        applyDepthRule()
    }

    var isEmptyText: Bool { lines.allSatisfy(\.isEmpty) }

    /// Rule 1 and rule 6: no line is deeper than the one above it plus one, the first line is
    /// at depth 0, and no line is below `maxDepth`. This runs after every change.
    mutating func applyDepthRule() {
        guard !lines.isEmpty else { return }
        lines[0].depth = 0
        for i in 1..<lines.count {
            lines[i].depth = max(0, min(lines[i].depth, min(lines[i - 1].depth + 1, Draft.maxDepth)))
        }
    }

    func index(of id: UUID) -> Int? { lines.firstIndex { $0.id == id } }

    /// Rule 2: the lines directly below `index` that are deeper than it.
    func subLineRange(of index: Int) -> Range<Int> {
        let depth = lines[index].depth
        var end = index + 1
        while end < lines.count && lines[end].depth > depth {
            end += 1
        }
        return (index + 1)..<end
    }

    func canGrow(_ index: Int) -> Bool {
        guard index >= 0, index < lines.count else { return false }
        if index == 0 { return false }
        if lines[index].depth >= Draft.maxDepth { return false }
        return lines[index].depth < lines[index - 1].depth + 1
    }

    func canShrink(_ index: Int) -> Bool {
        guard index >= 0, index < lines.count else { return false }
        return lines[index].depth > 0
    }

    /// Deeper: the active line and its sub-lines go one level down.
    mutating func deeper(_ index: Int) {
        guard canGrow(index) else { return }
        for i in index..<subLineRange(of: index).upperBound {
            lines[i].depth += 1
        }
    }

    /// Shallower: the active line and its sub-lines go one level up.
    mutating func shallower(_ index: Int) {
        guard canShrink(index) else { return }
        for i in index..<subLineRange(of: index).upperBound {
            lines[i].depth -= 1
        }
    }

    /// Rule 3: a new empty line directly below `index`, with the caret in it. Its depth is the
    /// active line's, or the depth of the line below when that line is deeper, so it becomes
    /// the first sub-line. It is a to-do when the active line is one (K10, AC-5.2).
    mutating func newLine(after index: Int) -> Int {
        let active = lines[index]
        let next = index + 1 < lines.count ? lines[index + 1].depth : active.depth
        let depth = max(active.depth, min(next, active.depth + 1))
        let line = DraftLine(depth: depth, todo: active.todo)
        lines.insert(line, at: index + 1)
        return index + 1
    }

    /// Rule 4: delete at the start of an empty line. Deeper lines go one level up; a
    /// top-level line is removed when another line is left, and the caret goes to the end of
    /// the line above (or the line below, when it was the first). Returns where the caret
    /// goes, or `nil` when nothing changed.
    mutating func deleteEmptyLine(at index: Int) -> (id: UUID, caretAtEnd: Bool)? {
        guard index >= 0, index < lines.count, lines[index].isEmpty else { return nil }
        if lines[index].depth > 0 {
            shallower(index)
            return (lines[index].id, true)
        }
        guard lines.count > 1 else { return nil }
        lines.remove(at: index)
        let target = index > 0 ? index - 1 : 0
        return (lines[target].id, true)
    }

    /// Rule 5: the lines that go to the core — empty lines dropped and the depth rule applied
    /// again (AC-3.5). The text of each line is the text the core will store, with its
    /// accepted links written as `[[Title]]`.
    func outgoing(resolve: (DraftLine) -> String) -> [OutlineLine] {
        var out: [OutlineLine] = []
        var depth = 0
        var started = false
        for line in lines where !line.isEmpty {
            let d = started ? min(line.depth, depth + 1) : 0
            out.append(OutlineLine(text: resolve(line), depth: UInt32(min(d, Draft.maxDepth)), todo: line.todo))
            depth = d
            started = true
        }
        return out
    }
}

/// Where the draft is kept between launches (rule 7, AC-1.5). Written whole after each
/// change: the file is small and an atomic write means it is never half a draft.
enum DraftStore {
    struct Saved: Codable {
        var version: Int = 1
        var lines: [DraftLine] = [DraftLine()]
    }

    static var url: URL {
        Paths.support.appendingPathComponent("draft.json")
    }

    static func load() -> Draft {
        guard let data = try? Data(contentsOf: url),
              let saved = try? JSONDecoder().decode(Saved.self, from: data),
              saved.version == 1
        else { return Draft() }
        var lines = saved.lines
        for i in lines.indices {
            for j in lines[i].links.indices where lines[i].links[j].day != nil && lines[i].links[j].words == nil {
                let link = lines[i].links[j]
                let utf16 = Array(lines[i].text.utf16)
                guard link.start >= 0, link.end <= utf16.count, link.start < link.end else { continue }
                lines[i].links[j].words = String(decoding: utf16[link.start..<link.end], as: UTF16.self)
            }
        }
        return Draft(lines: lines)
    }

    static func save(_ draft: Draft) {
        Paths.ensureSupportFolder()
        let saved = Saved(lines: draft.lines)
        guard let data = try? JSONEncoder().encode(saved) else { return }
        try? data.write(to: url, options: .atomic)
    }
}
