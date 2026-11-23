// Runs the generated Swift bindings against the real Rust core (no UI): the same calls the
// app makes. Built and run by `cargo run -p xtask -- mobile`.

import Foundation

func check(_ cond: Bool, _ what: String) {
    if !cond {
        FileHandle.standardError.write("FAIL: \(what)\n".data(using: .utf8)!)
        exit(1)
    }
    print("ok: \(what)")
}

let dir = FileManager.default.temporaryDirectory.appendingPathComponent("utopia-check-\(getpid())")
try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
let path = dir.appendingPathComponent("m.utopia").path
let g = try Graph.open(path: path, passphrase: nil)
try g.setToday(date: "2026-11-02")
let day = try g.today()
check(day.title == "November 2nd, 2026", "today's page from Swift")
check(!day.exists && day.blocks.isEmpty, "a visit makes no page (FR-1.2, AC-1.3)")
_ = try g.appendOutline(lines: [OutlineLine(text: "Call [[Dana]] from the phone", depth: 0, todo: false)])
_ = try g.appendOutline(lines: [
    OutlineLine(text: "Meeting with Dana", depth: 0, todo: false),
    OutlineLine(text: "Budget is approved", depth: 1, todo: true),
])
let written = try g.today()
check(written.exists, "the first Send makes the day")
check(written.blocks.count == 3, "the outline lands as blocks, one write")
check(written.blocks[2].depth == 1 && written.blocks[2].kind == "todo", "a line keeps its depth and its to-do")
check(written.blocks[0].parts.contains { $0.kind == .page }, "the link is a part the app can draw")
let dana = try g.findPage(title: "dana")
check(dana != nil, "a link created its page")
check(try g.backlinks(pageId: dana!.id).count == 1, "backlinks over FFI")
check(try g.searchTitles(query: "Da", limit: 5) == ["Dana"], "title search for [[")

// Suggestions and the text a link writes (AC-4.1, AC-4.11).
let matches = try g.titleSuggestions(text: "Lunch with dana", accepted: [], limit: 6)
check(matches.count == 1 && matches[0].title == "Dana", "a title in a line is suggested")
let stored = try g.textWithLinks(
    text: "Lunch with dana",
    links: [AcceptedLink(title: matches[0].title, start: matches[0].start, end: matches[0].end)]
)
check(stored == "Lunch with [[Dana]]", "an accepted link is written in the page's spelling")
let stale = try g.textWithLinks(
    text: "Lunch with Dan",
    links: [AcceptedLink(title: "Dana", start: 11, end: 15)]
)
check(stale == "Lunch with Dan", "a changed word is plain text")
let parts = try g.draftParts(
    text: "Lunch with dana",
    links: [AcceptedLink(title: "Dana", start: 11, end: 15)]
)
check(parts.contains { $0.kind == .page }, "a draft line draws its link")

// A tick is one command (AC-5.3), and a day's entries carry their time (K10, §4.4).
let todo = written.blocks.first { $0.kind == "todo" }!
try g.setTodoDone(blockId: todo.id, done: true)
check(try g.today().blocks.first { $0.id == todo.id }!.checked, "a to-do can be ticked over FFI")
check(try g.today().blocks.first { $0.id == todo.id }!.createdAtMs > 0, "a block carries its time")

// Search and recents (AC-8.1, AC-8.2).
let found = try g.search(words: "meeting", limit: 10)
check(found.blocks.contains { $0.pageTitle == "November 2nd, 2026" }, "a block is found by its words")
check(try g.recentPages(limit: 8).isEmpty == false, "a recent page is listed")

do {
    _ = try Graph.open(path: dir.appendingPathComponent("x").path, passphrase: "secret")
    // A new file with a key is created encrypted; reopening without one must fail.
    _ = try Graph.open(path: dir.appendingPathComponent("x").path, passphrase: nil)
    check(false, "an encrypted graph refuses to open without its passphrase")
} catch FfiError.Locked {
    check(true, "an encrypted graph refuses to open without its passphrase")
}
do {
    try g.setBlockText(blockId: "missing", text: "x")
    check(false, "errors cross the boundary")
} catch FfiError.NotFound {
    check(true, "errors cross the boundary as Swift errors")
}
// Reopen: everything was persisted.
let again = try Graph.open(path: path, passphrase: nil)
try again.setToday(date: "2026-11-02")
check(try again.today().blocks.count == 3, "durable across reopen")
print("all checks passed")
