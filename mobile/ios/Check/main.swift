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
    links: [AcceptedLink(title: matches[0].title, start: matches[0].start, end: matches[0].end, day: nil, words: nil)]
)
check(stored == "Lunch with [[Dana]]", "an accepted link is written in the page's spelling")
let stale = try g.textWithLinks(
    text: "Lunch with Dan",
    links: [AcceptedLink(title: "Dana", start: 11, end: 15, day: nil, words: nil)]
)
check(stale == "Lunch with Dan", "a changed word is plain text")
let parts = try g.draftParts(
    text: "Lunch with dana",
    links: [AcceptedLink(title: "Dana", start: 11, end: 15, day: nil, words: nil)]
)
check(parts.contains { $0.kind == .page }, "a draft line draws its link")

try g.setToday(date: "2026-11-03")
let dates = try g.dateSuggestions(text: "call Ana tomorrow", accepted: [], limit: 6)
check(dates.count == 1 && dates[0].words == "tomorrow", "a date phrase is a suggestion")
check(dates[0].title == "November 4th, 2026", "the chip names the day's title")
check(dates[0].day == "2026-11-04", "the chip carries the day it resolved to")
let dateLink = AcceptedLink(
    title: dates[0].title,
    start: dates[0].start,
    end: dates[0].end,
    day: dates[0].day,
    words: dates[0].words
)
let dateText = try g.textWithLinks(text: "call Ana tomorrow", links: [dateLink])
check(dateText == "call Ana [[November 4th, 2026]]", "an accepted date is written as its day's title")
let dateParts = try g.draftParts(text: "call Ana tomorrow", links: [dateLink])
check(dateParts.contains { $0.kind == .date && $0.text == "tomorrow" }, "a date link draws the typed words")
try g.setToday(date: "2026-11-10")
check(
    try g.textWithLinks(text: "call Ana tomorrow", links: [dateLink]) == "call Ana [[November 4th, 2026]]",
    "a Send a week later keeps the day the chip was tapped for"
)
let staleDate = try g.textWithLinks(
    text: "call Ana next fri",
    links: [
        AcceptedLink(
            title: dates[0].title,
            start: dates[0].start,
            end: dates[0].end,
            day: "2026-11-04",
            words: "tomorrow"
        )
    ]
)
check(staleDate == "call Ana next fri", "changed date words are plain text")
let grown = try g.textWithLinks(text: "call Ana tomorrows", links: [dateLink])
check(grown == "call Ana tomorrows", "a word that grows past the range drops the link")
let nextWeekLink = AcceptedLink(
    title: "November 10th, 2026",
    start: 0,
    end: 9,
    day: "2026-11-10",
    words: "next week"
)
let rebased = rebaseLinks(
    textBefore: "next week",
    editStart: 0,
    editRemovedLen: 0,
    replacement: "Call Ana ",
    links: [nextWeekLink]
)
check(
    rebased.count == 1 && rebased[0].start == 9 && rebased[0].end == 18,
    "editing the prefix moves the accepted range"
)
check(
    try g.textWithLinks(text: "Call Ana next week", links: rebased) == "Call Ana [[November 10th, 2026]]",
    "editing the prefix keeps the accepted day"
)
let emojiDate = AcceptedLink(
    title: "December 4th, 2026",
    start: 3,
    end: 12,
    day: "2026-12-04",
    words: "next week"
)
let emojiEdit = editBetween(
    old: "\u{1F600} next week",
    new: "\u{1F601} next week",
    links: [emojiDate]
)
check(
    emojiEdit.start == 0 && emojiEdit.removed == 2 && emojiEdit.replacement == "\u{1F601}",
    "an emoji edit is one scalar-wide edit"
)
let emojiRebased = rebaseLinks(
    textBefore: "\u{1F600} next week",
    editStart: emojiEdit.start,
    editRemovedLen: emojiEdit.removed,
    replacement: emojiEdit.replacement,
    links: [emojiDate]
)
check(
    emojiRebased.count == 1 && emojiRebased[0].start == 3 && emojiRebased[0].end == 12,
    "an emoji edit before an accepted date keeps its range"
)
check(
    try g.textWithLinks(text: "\u{1F601} next week", links: emojiRebased) == "\u{1F601} [[December 4th, 2026]]",
    "an emoji edit before an accepted date keeps the day"
)
check(
    try g.dateSuggestions(text: "`next week` and next week", accepted: [], limit: 6).map(\.words) == ["next week"],
    "a date inside inline code is no chip"
)
check(
    try g.dateSuggestions(text: "🙂 tomorrow", accepted: [], limit: 6).first?.start == 3,
    "a date after an emoji keeps UTF-16 offsets"
)
check(try g.dateSuggestions(text: "26", accepted: [], limit: 6).isEmpty, "a bare number is no date")
check(try g.dateSuggestions(text: "may", accepted: [], limit: 6).isEmpty, "a bare month is no date")
check(try g.dateSuggestions(text: "in May. 5 of us", accepted: [], limit: 6).isEmpty, "punctuation does not join a date")
try g.setToday(date: "2026-11-02")

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
