import SwiftUI

/// One top-level block with the blocks below it (§4.2 "Entry").
struct LogEntry: Identifiable {
    let head: BlockView
    let lines: [BlockView]

    var id: String { head.id }
}

extension PageView {
    /// The entries of a day, oldest first: each depth-0 block with its deeper rows below.
    var entries: [LogEntry] {
        var out: [LogEntry] = []
        var head: BlockView?
        var lines: [BlockView] = []
        for block in blocks {
            if block.depth == 0 {
                if let head {
                    out.append(LogEntry(head: head, lines: lines))
                }
                head = block
                lines = []
            } else {
                lines.append(block)
            }
        }
        if let head {
            out.append(LogEntry(head: head, lines: lines))
        }
        return out
    }
}

struct DayLog: View {
    @Environment(AppModel.self) private var model
    let openPage: (String) -> Void

    @ScaledMetric(relativeTo: .body) private var bodySize: CGFloat = 16
    @ScaledMetric(relativeTo: .body) private var childSize: CGFloat = 15
    @ScaledMetric(relativeTo: .body) private var lineHeight: CGFloat = 24
    @ScaledMetric(relativeTo: .body) private var childLineHeight: CGFloat = 22
    @ScaledMetric(relativeTo: .caption) private var labelSize: CGFloat = 12
    @ScaledMetric(relativeTo: .caption) private var timeWidth: CGFloat = 52

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    if let today = model.today {
                        if today.exists && !today.entries.isEmpty {
                            ForEach(today.entries) { entry in
                                row(entry)
                                    .id(entry.id)
                                rule
                            }
                        } else {
                            Text("Nothing for today yet.")
                                .font(.system(size: childSize))
                                .foregroundStyle(Tokens.muted)
                                .frame(maxWidth: .infinity)
                                .padding(.vertical, 40)
                        }
                        Color.clear.frame(height: 1).id("end")
                    }
                }
                .padding(.bottom, 8)
            }
            .scrollDismissesKeyboard(.never)
            .defaultScrollAnchor(.bottom)
            .environment(\.openURL, OpenURLAction { url in
                if let page = LinkScheme.page(url) {
                    openPage(page)
                    return .handled
                }
                return .systemAction
            })
            .onChange(of: model.lastSentEntry) { _, entry in
                guard let entry else { return }
                withAnimation(.easeOut(duration: 0.2)) {
                    proxy.scrollTo(entry, anchor: .bottom)
                }
            }
        }
    }

    private var rule: some View {
        Rectangle()
            .fill(Tokens.lineSoft)
            .frame(height: 1)
            .padding(.leading, 20)
            .padding(.trailing, 16)
    }

    private func row(_ entry: LogEntry) -> some View {
        HStack(alignment: .top, spacing: 0) {
            Text(time(entry.head.createdAtMs))
                .font(.label(labelSize))
                .foregroundStyle(Tokens.muted)
                .frame(width: timeWidth, alignment: .leading)
            VStack(alignment: .leading, spacing: 2) {
                headLine(entry.head)
                ForEach(entry.lines, id: \.id) { line in
                    childLine(line)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.top, 12)
        .padding(.bottom, 12)
        .padding(.leading, 20)
        .padding(.trailing, 16)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(entryAccessibility(entry))
    }

    private func headLine(_ block: BlockView) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            if block.placeholder != nil {
                BlockPlaceholderView(text: block.placeholder ?? "", size: bodySize)
                    .fontWeight(.medium)
            } else if block.kind == "todo" {
                HStack(alignment: .top, spacing: 0) {
                    TodoBox(done: block.checked, lineHeight: lineHeight) {
                        Task { await model.toggleTodo(block: block) }
                    }
                    text(block, size: bodySize, weight: .medium, colour: Tokens.ink)
                }
            } else {
                text(block, size: bodySize, weight: .medium, colour: Tokens.ink)
            }
        }
    }

    private func childLine(_ block: BlockView) -> some View {
        HStack(alignment: .top, spacing: 0) {
            Color.clear
                .frame(width: CGFloat(max(0, block.depth - 1)) * 16)
            if block.kind == "todo" {
                TodoBox(done: block.checked, lineHeight: childLineHeight) {
                    Task { await model.toggleTodo(block: block) }
                }
            } else {
                Circle()
                    .fill(Tokens.bullet)
                    .frame(width: 5, height: 5)
                    .frame(width: Metrics.childBulletColumn, height: childLineHeight, alignment: .leading)
            }
            if block.placeholder != nil {
                BlockPlaceholderView(text: block.placeholder ?? "", size: childSize)
            } else {
                text(block, size: childSize, weight: .regular, colour: Tokens.ink3)
            }
        }
    }

    @ViewBuilder
    private func text(_ block: BlockView, size: CGFloat, weight: Font.Weight, colour: Color) -> some View {
        let done = block.kind == "todo" && block.checked
        PartTextView(
            parts: block.parts,
            size: size,
            weight: weight,
            colour: done ? Tokens.muted : colour,
            struck: done
        )
    }

    private func time(_ ms: Int64) -> String {
        guard ms > 0 else { return "" }
        let date = Date(timeIntervalSince1970: Double(ms) / 1000)
        return date.formatted(date: .omitted, time: .shortened)
    }

    private func entryAccessibility(_ entry: LogEntry) -> String {
        let t = time(entry.head.createdAtMs)
        let head = entry.head.plain
        return t.isEmpty ? head : "\(t), \(head)"
    }
}

/// A 16 pt box in a 26 pt column, with a 44 × 44 pt touch target (G5, AC-5.5). The target
/// reaches to the leading side of the column, so a tap on the text never ticks the to-do.
struct TodoBox: View {
    let done: Bool
    let lineHeight: CGFloat
    let toggle: () -> Void

    private var topInset: CGFloat { max(0, (lineHeight - 16) / 2) }

    var body: some View {
        ZStack(alignment: .topLeading) {
            RoundedRectangle(cornerRadius: 4)
                .fill(done ? Tokens.link : Color.clear)
                .overlay(
                    RoundedRectangle(cornerRadius: 4)
                        .strokeBorder(done ? Tokens.link : Tokens.bullet, lineWidth: 1.5)
                )
                .frame(width: 16, height: 16)
                .overlay {
                    if done {
                        Image(systemName: "checkmark")
                            .font(.system(size: 10, weight: .bold))
                            .foregroundStyle(Tokens.paper)
                    }
                }
                .padding(.top, topInset)
            Button(action: toggle) {
                Color.clear
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .offset(x: 16 - 44, y: topInset - 11)
            .accessibilityLabel(done ? "Mark not done" : "Mark done")
        }
        .frame(width: Metrics.markColumn, height: lineHeight, alignment: .topLeading)
    }
}

#if DEBUG
#Preview("Day log") {
    DayLog { _ in }
        .environment(AppModel.preview())
        .background(Tokens.paper)
}

#Preview("Day log · Empty day") {
    DayLog { _ in }
        .environment(AppModel.preview(today: PreviewData.emptyToday))
        .background(Tokens.paper)
}

#Preview("To-do box") {
    @Previewable @State var done = false
    HStack(spacing: 24) {
        TodoBox(done: done, lineHeight: 24) { done.toggle() }
        TodoBox(done: true, lineHeight: 24) {}
        TodoBox(done: false, lineHeight: 24) {}
    }
    .padding(40)
}
#endif
