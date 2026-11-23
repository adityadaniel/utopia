// Drawing a block's text: the core gives the parts, already resolved (C1), and the view joins
// them with the link colour and a link for each page (ios-prd §4.4, AC-7.1).

import SwiftUI

enum LinkScheme {
    static let prefix = "utopia-page:"

    static func url(_ pageID: String) -> URL? {
        URL(string: prefix + pageID)
    }

    static func page(_ url: URL) -> String? {
        let text = url.absoluteString
        guard text.hasPrefix(prefix) else { return nil }
        return String(text.dropFirst(prefix.count))
    }
}

enum PartText {
    /// The parts joined into one text, each link in `link` and each page as a tappable link.
    static func attributed(_ parts: [TextPart]) -> AttributedString {
        var out = AttributedString()
        for part in parts {
            var run = AttributedString(part.text)
            switch part.kind {
            case .page, .tag, .date:
                run.foregroundColor = Tokens.link
                if let id = part.pageId, let url = LinkScheme.url(id) {
                    run.link = url
                    run.underlineStyle = .none
                }
            case .block, .link:
                run.foregroundColor = Tokens.link
            case .code, .plain:
                break
            }
            out += run
        }
        return out
    }
}

/// One line of a block that the phone draws as text.
struct PartTextView: View {
    let parts: [TextPart]
    var size: CGFloat = 15
    var weight: Font.Weight = .regular
    var colour: Color = Tokens.ink3
    var struck = false

    var body: some View {
        Text(PartText.attributed(parts))
            .font(.system(size: size, weight: weight))
            .foregroundStyle(colour)
            .tint(Tokens.link)
            .strikethrough(struck, color: Tokens.muted)
            .frame(maxWidth: .infinity, alignment: .leading)
            .fixedSize(horizontal: false, vertical: true)
    }
}

/// What a block type the phone cannot make reads as (AC-7.5): never raw markup.
struct BlockPlaceholderView: View {
    let text: String
    var size: CGFloat = 15

    var body: some View {
        Text(text)
            .font(.system(size: size))
            .foregroundStyle(Tokens.muted)
            .italic()
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}
