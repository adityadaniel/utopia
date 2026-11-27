// One line of the draft, edited in place. A text view is used only because the draft needs
// control the SwiftUI text fields do not give: the caret across lines, Return as "new line",
// delete at the start of an empty line, and link colouring (ios-prd §4.5; C9 allows a small
// `UIViewRepresentable` for the caret).

import SwiftUI
import UIKit

struct LineEditor: UIViewRepresentable {
    struct LinkSpan: Equatable {
        let kind: PartKind
        let start: UInt32
        let end: UInt32
    }

    let line: DraftLine
    let parts: [TextPart]
    let isActive: Bool
    let caret: CaretRequest?
    let size: CGFloat

    let onText: (String, Bool, LineEdit?) -> Void
    let onReturn: () -> Void
    let onDeleteAtStart: () -> Void
    let onPicker: () -> Void
    let onCaret: (Int) -> Void
    /// The line took the caret, however it happened: a tap in the text, or Return above it.
    let onFocus: () -> Void
    /// A paste of several lines: each line becomes a line of the draft (§7).
    let onPasteLines: ([String]) -> Void

    func makeUIView(context: Context) -> LineTextView {
        let view = LineTextView()
        view.delegate = context.coordinator
        view.isScrollEnabled = false
        view.isEditable = true
        view.backgroundColor = .clear
        view.textContainerInset = .zero
        view.textContainer.lineFragmentPadding = 0
        view.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        view.tintColor = UIColor(Tokens.link)
        view.onDeleteAtStart = { context.coordinator.onDeleteAtStart?() }
        context.coordinator.onDeleteAtStart = onDeleteAtStart
        context.coordinator.onPicker = onPicker
        context.coordinator.onPasteLines = onPasteLines
        view.onPasteLines = { lines in context.coordinator.onPasteLines?(lines) }
        return view
    }

    func updateUIView(_ view: LineTextView, context: Context) {
        let coordinator = context.coordinator
        coordinator.onDeleteAtStart = onDeleteAtStart
        coordinator.onPicker = onPicker
        coordinator.onPasteLines = onPasteLines
        coordinator.onFocus = onFocus
        coordinator.onText = onText
        coordinator.onReturn = onReturn
        coordinator.onCaret = onCaret
        coordinator.isUpdating = true
        defer { coordinator.isUpdating = false }

        let paragraph = NSMutableParagraphStyle()
        paragraph.lineSpacing = 0
        let attributes: [NSAttributedString.Key: Any] = [
            .font: UIFont.systemFont(ofSize: size),
            .foregroundColor: UIColor(Tokens.ink),
            .paragraphStyle: paragraph,
        ]
        let links = parts
            .filter { $0.kind != .plain }
            .map { LinkSpan(kind: $0.kind, start: $0.start, end: $0.end) }
        if view.markedTextRange == nil {
            if view.text != line.text {
                // Keep the caret where the reader put it: setting the text selects everything or
                // jumps to the end in a text view.
                coordinator.lastLinks = links
                let selection = view.selectedRange
                view.attributedText = Self.attributed(parts, text: line.text, attributes: attributes)
                let length = view.text.utf16.count
                view.selectedRange = NSRange(
                    location: min(selection.location, length),
                    length: min(selection.length, max(0, length - selection.location))
                )
            } else if coordinator.lastLinks != links {
                coordinator.lastLinks = links
                Self.paint(parts, in: view, attributes: attributes)
            }
        }
        view.typingAttributes = attributes
        view.textContainer.maximumNumberOfLines = 0
        view.textContainer.lineBreakMode = .byWordWrapping

        if isActive {
            if !view.isFirstResponder {
                view.becomeFirstResponder()
            }
            if let caret, caret.line == line.id, coordinator.appliedCaret != caret.token {
                coordinator.appliedCaret = caret.token
                let length = view.text.utf16.count
                let offset = min(max(0, caret.offset), length)
                view.selectedRange = NSRange(location: offset, length: 0)
            }
        } else if view.isFirstResponder {
            view.resignFirstResponder()
        }
    }

    func sizeThatFits(_ proposal: ProposedViewSize, uiView: LineTextView, context: Context) -> CGSize? {
        let width = proposal.width ?? uiView.bounds.width
        guard width > 0 else { return nil }
        let height = uiView.sizeThatFits(CGSize(width: width, height: .greatestFiniteMagnitude)).height
        return CGSize(width: width, height: max(22, ceil(height)))
    }

    func makeCoordinator() -> Coordinator {
        Coordinator()
    }

    /// The text with each accepted link in the link colour (AC-4.1). The parts are the core's
    /// (C1), and their offsets are UTF-16 offsets into the text, which is what `NSRange` is.
    private static func attributed(
        _ parts: [TextPart],
        text: String,
        attributes: [NSAttributedString.Key: Any]
    ) -> NSAttributedString {
        let full = NSMutableAttributedString(string: text, attributes: attributes)
        let length = (text as NSString).length
        for part in parts where part.kind == .page || part.kind == .date || part.kind == .tag {
            let range = NSRange(location: Int(part.start), length: Int(part.end - part.start))
            guard range.location >= 0, range.location + range.length <= length else { continue }
            full.addAttribute(.foregroundColor, value: UIColor(Tokens.link), range: range)
        }
        return full
    }

    private static func paint(
        _ parts: [TextPart],
        in view: LineTextView,
        attributes: [NSAttributedString.Key: Any]
    ) {
        let storage = view.textStorage
        let length = storage.length
        storage.beginEditing()
        storage.setAttributes(attributes, range: NSRange(location: 0, length: length))
        for part in parts where part.kind == .page || part.kind == .date || part.kind == .tag {
            let range = NSRange(location: Int(part.start), length: Int(part.end - part.start))
            guard range.location >= 0, range.location + range.length <= length else { continue }
            storage.addAttribute(.foregroundColor, value: UIColor(Tokens.link), range: range)
        }
        storage.endEditing()
    }

    final class Coordinator: NSObject, UITextViewDelegate {
        var onText: ((String, Bool, LineEdit?) -> Void)?
        var onReturn: (() -> Void)?
        var onDeleteAtStart: (() -> Void)?
        var onPicker: (() -> Void)?
        var onCaret: ((Int) -> Void)?
        var onFocus: (() -> Void)?
        var onPasteLines: (([String]) -> Void)?
        var appliedCaret: UUID?
        var lastLinks: [LinkSpan] = []
        var isUpdating = false
        var pendingEdit: LineEdit?
        var wasComposing = false

        func textViewDidChange(_ textView: UITextView) {
            guard !isUpdating else { return }
            let edit = pendingEdit
            pendingEdit = nil
            onCaret?(textView.selectedRange.location)
            wasComposing = textView.markedTextRange != nil
            onText?(textView.text, wasComposing, edit)
        }

        func textViewDidBeginEditing(_ textView: UITextView) {
            onFocus?()
        }

        func textViewDidChangeSelection(_ textView: UITextView) {
            guard !isUpdating else { return }
            onCaret?(textView.selectedRange.location)
            let composing = textView.markedTextRange != nil
            if wasComposing && !composing {
                wasComposing = false
                onText?(textView.text, false, nil)
            }
        }

        /// Return makes a new line of the draft; it never sends (§4.5 rule 3, AC-1.6).
        func textView(
            _ textView: UITextView,
            shouldChangeTextIn range: NSRange,
            replacementText text: String
        ) -> Bool {
            pendingEdit = nil
            if text == "\n" {
                onReturn?()
                return false
            }
            // Typing `[[` or `#` opens the page picker and the typed characters are removed
            // (AC-4.7).
            let ns = textView.text as NSString
            if text == "[" {
                let previous = range.location > 0 ? ns.substring(with: NSRange(location: range.location - 1, length: 1)) : ""
                if previous == "[" {
                    // The first bracket is taken away too, so the typed characters are gone
                    // (AC-4.7); the model is told, because this change is not a keystroke.
                    textView.text = ns.replacingCharacters(in: NSRange(location: range.location - 1, length: 1), with: "")
                    let caret = max(0, range.location - 1)
                    textView.selectedRange = NSRange(location: caret, length: 0)
                    onCaret?(caret)
                    onText?(textView.text, false, LineEdit(start: range.location - 1, removed: 1, replacement: ""))
                    onPicker?()
                    return false
                }
            }
            if text == "#" {
                let before = range.location > 0 ? ns.substring(with: NSRange(location: range.location - 1, length: 1)) : " "
                if before.trimmingCharacters(in: .whitespaces).isEmpty || range.location == 0 {
                    onPicker?()
                    return false
                }
            }
            pendingEdit = LineEdit(start: range.location, removed: range.length, replacement: text)
            return true
        }
    }
}

/// A text view that deletes at the start of an empty line through the draft's own rules
/// (§4.5 rule 4) instead of doing nothing.
final class LineTextView: UITextView {
    var onDeleteAtStart: (() -> Void)?
    var onPasteLines: (([String]) -> Void)?

    /// Pasting several lines makes several lines of the draft, not one block with breaks in it
    /// (ios-prd §7: each pasted line is one line).
    override func paste(_ sender: Any?) {
        let text = UIPasteboard.general.string ?? ""
        guard text.contains("\n") else {
            super.paste(sender)
            return
        }
        onPasteLines?(text.replacingOccurrences(of: "\r\n", with: "\n").components(separatedBy: "\n"))
    }

    override func deleteBackward() {
        // At the start of a line the draft's own rule decides what happens (§4.5 rule 4);
        // a line with text simply stays as it is.
        if selectedRange.location == 0 && selectedRange.length == 0 {
            onDeleteAtStart?()
            return
        }
        super.deleteBackward()
    }
}
