// The "Quiet paper" tokens of DESIGN.md §2.1, under the same names as the Mac, with the
// dark values from the same table. A view names a token and never a colour (ios-prd §4.7,
// C8). Sizes come from the ios-prd §4 tables and follow Dynamic Type through @ScaledMetric
// in the views that draw them.

import SwiftUI
import UIKit

enum Tokens {
    static let paper = dynamic(0xFFFFFF, 0x1B1B19)
    static let shelf = dynamic(0xF6F6F4, 0x151514)
    static let well = dynamic(0xFAFAF8, 0x202020)
    static let selected = dynamic(0xE9E9E5, 0x2C2C29)
    static let line = dynamic(0xE8E7E3, 0x33332F)
    static let lineSoft = dynamic(0xEFEEEA, 0x2A2A27)
    static let lineStrong = dynamic(0xE1E0DB, 0x3D3D38)
    static let ink = dynamic(0x1B1B19, 0xE9E8E3)
    static let ink3 = dynamic(0x57574F, 0xA3A29A)
    static let muted = dynamic(0x6C6C65, 0x8F8E86)
    static let bullet = dynamic(0x8A8A83, 0x6E6D66)
    static let disabled = dynamic(0xB9B8B1, 0x55554F)
    static let link = dynamic(0x2D5BD2, 0x7FA3F0)
    static let linkHover = dynamic(0x2249B0, 0x9DB9F5)
    static let linkTint = dynamic(0xEAEFFB, 0x262F45)
    static let success = dynamic(0x1F7A3D, 0x6FD08C)
    static let warning = dynamic(0x9A5A00, 0xF0B45A)
    static let danger = dynamic(0xB42318, 0xFF9C94)

    /// A `--mark` on a `--link` fill is `--paper`, never white: `DESIGN.md` §2.1.
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
