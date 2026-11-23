// The colours of the phone. The one accent is the blue `--link` of DESIGN.md §2.1, with
// its light and dark values: links, the caret, the Send button and a control that is on.
// Every other colour is an iOS system colour, so the app follows the system's light, dark
// and Increase Contrast looks (ios-prd §4.7). The names stay the Mac token names, so a view
// still names a token and never a colour. Sizes come from the ios-prd §4 tables and follow
// Dynamic Type through @ScaledMetric in the views that draw them.

import SwiftUI
import UIKit

enum Tokens {
    // MARK: The accent (DESIGN.md §2.1)

    /// `--link`: links, the text caret, the Send button, a done to-do, a control that is on.
    static let link = dynamic(0x2D5BD2, 0x7FA3F0)
    /// `--link-tint`: the fill behind a control that is on.
    static let linkTint = dynamic(0xEAEFFB, 0x262F45)

    // MARK: System colours

    /// The screen behind the log and the pages.
    static let paper = Color(uiColor: .systemBackground)
    /// The compose box.
    static let shelf = Color(uiColor: .secondarySystemBackground)
    /// The panels that take the keyboard's place.
    static let well = Color(uiColor: .secondarySystemBackground)
    /// The fill of a control that cannot act now (Send with an empty draft).
    static let selected = Color(uiColor: .tertiarySystemFill)
    static let line = Color(uiColor: .separator)
    static let lineSoft = Color(uiColor: .separator)
    static let lineStrong = Color(uiColor: .opaqueSeparator)
    /// Primary text.
    static let ink = Color(uiColor: .label)
    /// Secondary text: the lines below an entry, row notes.
    static let ink3 = Color(uiColor: .secondaryLabel)
    /// Labels, times, empty states, a done to-do.
    static let muted = Color(uiColor: .secondaryLabel)
    /// The hint in an empty line of the draft.
    static let placeholder = Color(uiColor: .placeholderText)
    /// Bullets and to-do borders. Never text.
    static let bullet = Color(uiColor: .systemGray2)
    /// A control that cannot act, the offline dot. Never text.
    static let disabled = Color(uiColor: .tertiaryLabel)
    static let success = Color(uiColor: .systemGreen)
    static let warning = Color(uiColor: .systemOrange)
    static let danger = Color(uiColor: .systemRed)

    /// A colour with a light and a dark value, for the accent only.
    static func dynamic(_ light: UInt32, _ dark: UInt32) -> Color {
        Color(uiColor: UIColor { traits in
            UIColor(rgb: traits.userInterfaceStyle == .dark ? dark : light)
        })
    }
}

extension UIColor {
    convenience init(rgb: UInt32) {
        self.init(
            red: CGFloat((rgb >> 16) & 0xFF) / 255,
            green: CGFloat((rgb >> 8) & 0xFF) / 255,
            blue: CGFloat(rgb & 0xFF) / 255,
            alpha: 1
        )
    }
}

/// The sizes of §4.3–§4.7 at the default text size. Each view scales the ones it draws with
/// `@ScaledMetric`, so the layout follows Dynamic Type.
enum Metrics {
    static let sideMargin: CGFloat = 12
    static let boxRadius: CGFloat = 24
    static let markColumn: CGFloat = 26
    static let indent: CGFloat = 20
    static let control: CGFloat = 44
    static let icon: CGFloat = 20
    static let getLoose: CGFloat = 44
    static let timeColumn: CGFloat = 52
    static let childBulletColumn: CGFloat = 16
    static let todoBox: CGFloat = 16
    /// About three lines of the default 22 pt line, then the lines area scrolls (§4.5).
    static let linesAreaLimit: CGFloat = 138
}

extension Font {
    /// The label font (§4.7): the system monospaced font, used for weekday, times and row
    /// notes.
    static func label(_ size: CGFloat) -> Font {
        .system(size: size, weight: .medium, design: .monospaced)
    }
}
