// The compose box (ios-prd §4.5): a filled `shelf` rounded rectangle with 24 pt corners, the
// lines of the draft, and an icon-only action bar below them. Nesting is a property of a
// line, so a phone needs no Tab key (K2, G3).

import SwiftUI

struct ComposeBox: View {
    @Environment(AppModel.self) private var model

    @ScaledMetric(relativeTo: .body) private var textSize: CGFloat = 16
    @ScaledMetric(relativeTo: .body) private var rowHeight: CGFloat = 44
    @ScaledMetric(relativeTo: .body) private var lineHeight: CGFloat = 22
    @ScaledMetric(relativeTo: .body) private var linesLimit: CGFloat = 138
    @ScaledMetric(relativeTo: .body) private var markSize: CGFloat = 6
    @ScaledMetric(relativeTo: .body) private var todoSize: CGFloat = 16
    @ScaledMetric(relativeTo: .body) private var iconSize: CGFloat = 20
    @ScaledMetric(relativeTo: .body) private var indent: CGFloat = 20
    @ScaledMetric(relativeTo: .body) private var markColumn: CGFloat = 26

    @State private var contentHeight: CGFloat = 44

    /// The room a one-line text needs inside a 44 pt row, so the row is exactly 44 pt high and
    /// a wrapped line makes it grow (§4.5: "a line is at least 44 pt high and wraps").
    private var textInset: CGFloat { max(0, (rowHeight - lineHeight) / 2) }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            lines
            actionBar
        }
        .background(
            RoundedRectangle(cornerRadius: Metrics.boxRadius, style: .continuous)
                .fill(Tokens.shelf)
        )
    }

    private var lines: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(model.draft.lines) { line in
                        lineRow(line)
                            .id(line.id)
                    }
                }
                .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { height in
                    contentHeight = max(rowHeight, height)
                }
            }
            .frame(height: min(contentHeight, linesLimit))
            .scrollIndicators(.hidden)
            .scrollBounceBehavior(.basedOnSize)
            .onChange(of: model.draft.lines.count) { _, _ in
                guard let last = model.activeLine else { return }
                withAnimation(.easeOut(duration: 0.15)) {
                    proxy.scrollTo(last, anchor: .bottom)
                }
            }
            .onChange(of: model.caretRequest) { _, request in
                guard let request else { return }
                withAnimation(.easeOut(duration: 0.15)) {
                    proxy.scrollTo(request.line, anchor: .bottom)
                }
            }
        }
        .padding(.top, 6)
        .padding(.leading, 10)
        .padding(.trailing, 14)
    }

    private func lineRow(_ line: DraftLine) -> some View {
        HStack(alignment: .top, spacing: 0) {
            Color.clear
                .frame(width: CGFloat(line.depth) * indent, height: rowHeight)
            mark(line)
            ZStack(alignment: .topLeading) {
                if line.text.isEmpty {
                    Text(placeholder(line))
                        .font(.system(size: textSize))
                        .foregroundStyle(Tokens.placeholder)
                        .padding(.top, textInset)
                        .allowsHitTesting(false)
                }
                LineEditor(
                    line: line,
                    parts: model.lineParts[line.id] ?? [],
                    // An open picker takes the keyboard's place, so the line lets the keyboard
                    // go (§4.6); the caret comes back when the picker closes.
                    isActive: model.activeLine == line.id && model.picker == nil,
                    caret: model.caretRequest,
                    size: textSize,
                    onText: { text in model.setText(text, for: line.id) },
                    onReturn: { _ = model.newLine(after: line.id) },
                    onDeleteAtStart: { model.deleteEmptyLine(line.id) },
                    onPicker: {
                        model.focusLine(line.id, at: nil)
                        model.showPicker(.link)
                    },
                    onCaret: { offset in
                        if model.activeLine != line.id {
                            model.focusLine(line.id, at: nil)
                        }
                        _ = offset
                    },
                    onFocus: { model.focusLine(line.id, at: nil) },
                    onPasteLines: { lines in model.pasteLines(lines, after: line.id) }
                )
                .padding(.vertical, textInset)
            }
        }
        .frame(minHeight: rowHeight)
    }

    @ViewBuilder
    private func mark(_ line: DraftLine) -> some View {
        Group {
            if line.todo {
                RoundedRectangle(cornerRadius: 4)
                    .strokeBorder(Tokens.bullet, lineWidth: 1.5)
                    .frame(width: todoSize, height: todoSize)
            } else {
                Circle()
                    .fill(Tokens.bullet)
                    .frame(width: markSize, height: markSize)
            }
        }
        .frame(width: markColumn, height: rowHeight)
        .accessibilityHidden(true)
    }

    private func placeholder(_ line: DraftLine) -> String {
        if line.id == model.draft.lines.first?.id {
            return "Add to today"
        }
        return line.todo ? "Next to-do" : "Next line"
    }

    private var actionBar: some View {
        HStack(spacing: 2) {
            control("Shallower", symbol: "decrease.indent", enabled: model.canShrinkActive, on: false) {
                model.shallower()
            }
            control("Deeper", symbol: "increase.indent", enabled: model.canGrowActive, on: false) {
                model.deeper()
            }
            control("Link a page", symbol: "link", enabled: true, on: model.picker == .link) {
                if model.picker == .link {
                    model.closePicker()
                } else {
                    model.showPicker(.link)
                }
            }
            control("To-do", symbol: "checkmark.square", enabled: true, on: model.activeIsTodo) {
                model.toggleTodo()
            }
            control("Add a date", symbol: "calendar", enabled: true, on: model.picker == .date) {
                if model.picker == .date {
                    model.closePicker()
                } else {
                    model.showPicker(.date)
                }
            }
            Spacer(minLength: 0)
            sendButton
        }
        .padding(.horizontal, 8)
        .padding(.bottom, 8)
        .padding(.top, 2)
    }

    private func control(
        _ label: String,
        symbol: String,
        enabled: Bool,
        on: Bool,
        action: @escaping () -> Void
    ) -> some View {
        Button {
            guard enabled else { return }
            action()
        } label: {
            Image(systemName: symbol)
                .font(.system(size: iconSize))
                .foregroundStyle(enabled ? (on ? Tokens.link : Tokens.ink3) : Tokens.disabled)
                .frame(width: Metrics.control, height: Metrics.control)
                .background(
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .fill(on ? Tokens.linkTint : Color.clear)
                )
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
        .accessibilityLabel(label)
        // A plain SwiftUI button reports itself as selected here whatever the state is; the
        // trait is set from the control's own state so VoiceOver reads it correctly.
        .accessibilityRemoveTraits(.isSelected)
        .accessibilityAddTraits(on ? .isSelected : [])
        .accessibilityValue(on ? "On" : "Off")
    }

    private var sendButton: some View {
        Button {
            Task { await model.send() }
        } label: {
            Image(systemName: "arrow.up")
                .font(.system(size: iconSize, weight: .semibold))
                .foregroundStyle(model.canSend ? Tokens.paper : Tokens.muted)
                .frame(width: Metrics.control, height: Metrics.control)
                .background(
                    Circle().fill(model.canSend ? Tokens.link : Tokens.selected)
                )
        }
        .buttonStyle(.plain)
        .disabled(!model.canSend)
        .accessibilityLabel("Add to today")
    }
}

/// The chips above the box: one for each title the core found in the active line (§4.6).
/// A tap accepts the link and leaves the words as they are (AC-4.1, AC-4.4).
struct SuggestionRow: View {
    @Environment(AppModel.self) private var model

    @ScaledMetric(relativeTo: .body) private var textSize: CGFloat = 15
    @ScaledMetric(relativeTo: .body) private var iconSize: CGFloat = 14

    var body: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 8) {
                ForEach(model.suggestions, id: \.pageId) { match in
                    Button {
                        model.acceptSuggestion(match)
                    } label: {
                        HStack(spacing: 6) {
                            Image(systemName: "link")
                                .font(.system(size: iconSize))
                                .foregroundStyle(Tokens.ink3)
                            Text("Link \(Text(match.title).bold())")
                                .font(.system(size: textSize))
                                .foregroundStyle(Tokens.ink)
                        }
                        .padding(.horizontal, 12)
                        .frame(height: 44)
                        .background(
                            RoundedRectangle(cornerRadius: 8, style: .continuous)
                                .fill(Tokens.paper)
                                .overlay(
                                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                                        .strokeBorder(Tokens.lineStrong, lineWidth: 1)
                                )
                        )
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Link \(match.title)")
                }
            }
            .padding(.horizontal, 2)
        }
        .scrollIndicators(.hidden)
    }
}

#if DEBUG
#Preview("Compose box") {
    ComposeBox()
        .padding(12)
        .environment(AppModel.preview())
}

#Preview("Compose box · Empty") {
    ComposeBox()
        .padding(12)
        .environment(AppModel.preview(draft: PreviewData.emptyDraft))
}

#Preview("Compose box · Dark") {
    ComposeBox()
        .padding(12)
        .frame(maxHeight: .infinity, alignment: .bottom)
        .background(Tokens.paper)
        .environment(AppModel.preview())
        .preferredColorScheme(.dark)
}

#Preview("Suggestion chips") {
    SuggestionRow()
        .padding(12)
        .environment(AppModel.preview(suggestions: PreviewData.suggestions))
}
#endif
