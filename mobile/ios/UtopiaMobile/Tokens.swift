import SwiftUI
import UIKit

enum Tokens {

    static let link = dynamic(0x2D5BD2, 0x7FA3F0)
    static let linkTint = dynamic(0xEAEFFB, 0x262F45)

    static let paper = Color(uiColor: .systemBackground)
    static let shelf = Color(uiColor: .secondarySystemBackground)
    static let well = Color(uiColor: .secondarySystemBackground)
    static let selected = Color(uiColor: .tertiarySystemFill)
    static let line = Color(uiColor: .separator)
    static let lineSoft = Color(uiColor: .separator)
    static let lineStrong = Color(uiColor: .opaqueSeparator)
    static let ink = Color(uiColor: .label)
    static let ink3 = Color(uiColor: .secondaryLabel)
    static let muted = Color(uiColor: .secondaryLabel)
    static let placeholder = Color(uiColor: .placeholderText)
    static let bullet = Color(uiColor: .systemGray2)
    static let disabled = Color(uiColor: .tertiaryLabel)
    static let success = Color(uiColor: .systemGreen)
    static let warning = Color(uiColor: .systemOrange)
    static let danger = Color(uiColor: .systemRed)

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
