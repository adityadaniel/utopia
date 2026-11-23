// Mock data for the SwiftUI previews: a small graph with today, yesterday and a few pages
// that link to each other. Debug builds only; the app never reads it.

#if DEBUG
import Foundation

enum PreviewData {
    // MARK: - Pages

    static let todayID = "page-today"
    static let yesterdayID = "page-yesterday"
    static let budgetID = "page-budget"
    static let danaID = "page-dana"
    static let projectID = "page-utopia"

    static let today = PageView(
        id: todayID,
        title: "Nov 23rd, 2026",
        date: "2026-11-23",
        blocks: [
            block("t1", at: 8, 12, [.plain("Coffee with "), .page("Dana", danaID)]),
            block("t1a", depth: 1, at: 8, 14, [.plain("She thinks the "), .page("Budget", budgetID), .plain(" is too tight")]),
            block("t1b", depth: 1, at: 8, 15, [.plain("Ask about the Q1 hiring plan")], kind: "todo"),
            block("t2", at: 10, 40, [.plain("Ship the iPhone preview build")], kind: "todo", checked: true),
            block("t3", at: 13, 5, [.plain("Ideas for "), .page("Utopia", projectID), .plain(" "), .tag("#mobile", projectID)]),
            block("t3a", depth: 1, at: 13, 6, [.plain("Pin the log to today")]),
            block("t3b", depth: 2, at: 13, 7, [.plain("Earlier days open from Find")]),
            block("t4", at: 16, 30, [], placeholder: "A code block"),
            block("t5", at: 18, 2, [.plain("A long thought that wraps over more than one line, so the row grows and the time stays at the top of it.")]),
        ],
        properties: [],
        exists: true
    )

    static let emptyToday = PageView(
        id: todayID, title: "Nov 23rd, 2026", date: "2026-11-23", blocks: [], properties: [], exists: false
    )

    static let yesterday = PageView(
        id: yesterdayID,
        title: "Nov 22nd, 2026",
        date: "2026-11-22",
        blocks: [
            block("y1", at: 9, 0, [.plain("Draft the "), .page("Budget", budgetID), .plain(" for next year")]),
            block("y2", at: 15, 20, [.plain("Call the bank")], kind: "todo"),
        ],
        properties: [],
        exists: true
    )

    static let budget = PageView(
        id: budgetID,
        title: "Budget",
        date: nil,
        blocks: [
            block("b0", at: 9, 0, [.plain("owner:: Dana")], property: "owner"),
            block("b1", at: 9, 1, [.plain("Fixed costs")]),
            block("b1a", depth: 1, at: 9, 2, [.plain("Rent and the relay server")]),
            block("b1b", depth: 1, at: 9, 3, [.plain("Check the yearly plan price")], kind: "todo"),
            block("b2", at: 9, 4, [.plain("Hiring, see "), .date("Nov 22nd, 2026", yesterdayID)]),
            block("b3", at: 9, 5, [], placeholder: "A query"),
        ],
        properties: [
            PropertyView(key: "owner", value: "Dana"),
            PropertyView(key: "status", value: "Draft"),
        ],
        exists: true
    )

    static let dana = PageView(
        id: danaID,
        title: "Dana",
        date: nil,
        blocks: [block("d1", at: 9, 0, [.plain("Works on the money side of "), .page("Utopia", projectID)])],
        properties: [],
        exists: true
    )

    static let project = PageView(
        id: projectID, title: "Utopia", date: nil, blocks: [], properties: [], exists: true
    )

    static let budgetReferences: [RefView] = [
        RefView(pageId: todayID, pageTitle: today.title, blockId: "t1a", text: "She thinks the Budget is too tight"),
        RefView(pageId: yesterdayID, pageTitle: yesterday.title, blockId: "y1", text: "Draft the Budget for next year"),
        RefView(pageId: danaID, pageTitle: "Dana", blockId: "d1", text: "Owns the Budget this year"),
    ]

    static let recent: [PageRef] = [
        PageRef(id: budgetID, title: "Budget", date: nil),
        PageRef(id: danaID, title: "Dana", date: nil),
        PageRef(id: projectID, title: "Utopia", date: nil),
    ]

    static let fixture = PreviewFixture(
        pages: [
            todayID: today, yesterdayID: yesterday, budgetID: budget, danaID: dana, projectID: project,
        ],
        backlinks: [budgetID: budgetReferences],
        recent: recent
    )

    // MARK: - The draft

    static let draft = Draft(lines: [
        DraftLine(text: "Meeting with Dana about the budget"),
        DraftLine(text: "Move the launch", depth: 1),
        DraftLine(text: "", depth: 1, todo: true),
    ])

    static let emptyDraft = Draft()

    static let suggestions: [TitleMatch] = [
        TitleMatch(title: "Dana", pageId: danaID, start: 13, end: 17),
        TitleMatch(title: "Budget", pageId: budgetID, start: 28, end: 34),
    ]

    // MARK: - Building blocks

    enum Piece {
        case plain(String)
        case page(String, String)
        case tag(String, String)
        case date(String, String)

        var kind: PartKind {
            switch self {
            case .plain: .plain
            case .page: .page
            case .tag: .tag
            case .date: .date
            }
        }

        var text: String {
            switch self {
            case let .plain(text), let .page(text, _), let .tag(text, _), let .date(text, _): text
            }
        }

        var pageID: String? {
            switch self {
            case .plain: nil
            case let .page(_, id), let .tag(_, id), let .date(_, id): id
            }
        }
    }

    /// The parts of a block's text, with UTF-16 ranges like the core gives them.
    static func parts(_ pieces: [Piece]) -> [TextPart] {
        var offset: UInt32 = 0
        return pieces.map { piece in
            let length = UInt32(piece.text.utf16.count)
            defer { offset += length }
            return TextPart(kind: piece.kind, text: piece.text, pageId: piece.pageID, start: offset, end: offset + length)
        }
    }

    static func block(
        _ id: String,
        depth: UInt32 = 0,
        at hour: Int,
        _ minute: Int,
        _ pieces: [Piece],
        kind: String = "bullet",
        checked: Bool = false,
        placeholder: String? = nil,
        property: String? = nil
    ) -> BlockView {
        let plain = placeholder ?? pieces.map(\.text).joined()
        return BlockView(
            id: id,
            text: plain,
            plain: plain,
            depth: depth,
            kind: kind,
            headingLevel: 0,
            checked: checked,
            createdAtMs: time(hour, minute),
            parts: parts(pieces),
            placeholder: placeholder,
            propertyKey: property
        )
    }

    /// A time on 2026-11-23 in the device's time zone, in milliseconds.
    static func time(_ hour: Int, _ minute: Int) -> Int64 {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = .current
        let date = calendar.date(from: DateComponents(year: 2026, month: 11, day: 23, hour: hour, minute: minute)) ?? .now
        return Int64(date.timeIntervalSince1970 * 1000)
    }
}
#endif
