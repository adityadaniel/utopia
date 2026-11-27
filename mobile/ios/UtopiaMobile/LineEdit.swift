import Foundation

struct LineEdit: Sendable {
    let start: Int
    let removed: Int
    let replacement: String

    func applied(to text: String) -> String? {
        let ns = text as NSString
        guard start >= 0, removed >= 0, start + removed <= ns.length else { return nil }
        return ns.replacingCharacters(
            in: NSRange(location: start, length: removed),
            with: replacement
        )
    }

    func matches(_ text: String, oldText: String) -> Bool {
        guard let applied = applied(to: oldText) else { return false }
        return Array(applied.utf16) == Array(text.utf16)
    }
}
