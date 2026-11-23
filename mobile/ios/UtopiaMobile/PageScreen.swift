// A page, read-only (ios-prd IO-7): its properties, its outline and its Linked References,
// grouped by source page with a count. There is no compose box here (AC-7.4).

import SwiftUI

struct PageScreen: View {
    @Environment(AppModel.self) private var model
    let route: Route
    var open: (Route) -> Void

    @State private var page: PageView?
    @State private var references: [(pageTitle: String, hits: [RefView])] = []
    @State private var loaded = false

    @ScaledMetric(relativeTo: .callout) private var sectionSize: CGFloat = 13
    @ScaledMetric(relativeTo: .body) private var bodySize: CGFloat = 16
    @ScaledMetric(relativeTo: .body) private var childSize: CGFloat = 15
    @ScaledMetric(relativeTo: .caption) private var labelSize: CGFloat = 12
    @ScaledMetric(relativeTo: .body) private var lineHeight: CGFloat = 22

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ScrollViewReader { proxy in
                ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    if let page {
                        ForEach(page.properties, id: \.key) { property in
                            HStack(alignment: .firstTextBaseline, spacing: 8) {
                                Text(property.key)
                                    .font(.label(labelSize))
                                    .foregroundStyle(Tokens.muted)
                                    .frame(minWidth: 80, alignment: .leading)
                                Text(property.value)
                                    .font(.system(size: childSize))
                                    .foregroundStyle(Tokens.ink3)
                            }
                            .padding(.horizontal, 20)
                            .padding(.vertical, 4)
                        }
                        if !page.properties.isEmpty {
                            Rectangle()
                                .fill(Tokens.lineSoft)
                                .frame(height: 1)
                                .padding(.vertical, 8)
                        }
                        ForEach(page.blocks.filter { $0.propertyKey == nil }, id: \.id) { block in
                            blockRow(block)
                                .id(block.id)
                        }
                        if page.blocks.isEmpty {
                            Text("Nothing on this page yet.")
                                .font(.system(size: childSize))
                                .foregroundStyle(Tokens.muted)
                                .padding(.horizontal, 20)
                                .padding(.vertical, 16)
                        }
                        referencesSection(page)
                    } else if loaded {
                        Text("This page has no notes.")
                            .font(.system(size: childSize))
                            .foregroundStyle(Tokens.muted)
                            .padding(.horizontal, 20)
                            .padding(.vertical, 16)
                    }
                }
                .padding(.bottom, 24)
                }
                .task(id: route.pageID) {
                    page = await model.page(route.pageID)
                    references = await model.backlinks(route.pageID)
                    loaded = true
                    guard let block = route.blockID else { return }
                    // A reference opens its source page with that block in view (AC-7.2).
                    try? await Task.sleep(for: .milliseconds(60))
                    withAnimation(nil) {
                        proxy.scrollTo(block, anchor: .top)
                    }
                }
            }
            .environment(\.openURL, OpenURLAction { url in
                if let target = LinkScheme.page(url) {
                    open(Route(pageID: target))
                    return .handled
                }
                return .systemAction
            })
        }
        .background(Tokens.paper)
        // The system bar gives the Back button and the title (IO-7); the title stays one
        // line when the page scrolls.
        .navigationTitle(page?.title ?? "")
        .toolbarTitleDisplayMode(.inlineLarge)
    }


    @ViewBuilder
    private func blockRow(_ block: BlockView) -> some View {
        HStack(alignment: .top, spacing: 0) {
            Color.clear.frame(width: CGFloat(block.depth) * 16)
            if block.kind == "todo" {
                // A to-do has its box at every depth; it is the one thing the phone may change
                // on a block that is already in the graph (K9).
                TodoBox(done: block.checked, lineHeight: lineHeight) {
                    Task { await model.toggleTodo(block: block) }
                }
            } else if block.depth > 0 {
                Circle()
                    .fill(Tokens.bullet)
                    .frame(width: 5, height: 5)
                    .frame(width: Metrics.childBulletColumn, height: lineHeight, alignment: .leading)
            }
            if block.placeholder != nil {
                BlockPlaceholderView(text: block.placeholder ?? "", size: block.depth == 0 ? bodySize : childSize)
                    .padding(.vertical, 4)
            } else {
                PartTextView(
                    parts: block.parts,
                    size: block.depth == 0 ? bodySize : childSize,
                    weight: block.depth == 0 ? .medium : .regular,
                    colour: block.kind == "todo" && block.checked ? Tokens.muted : (block.depth == 0 ? Tokens.ink : Tokens.ink3),
                    struck: block.kind == "todo" && block.checked
                )
                .padding(.vertical, 4)
            }
        }
        .padding(.leading, 20)
        .padding(.trailing, 16)
    }

    @ViewBuilder
    private func referencesSection(_ page: PageView) -> some View {
        let total = references.reduce(0) { $0 + $1.hits.count }
        VStack(alignment: .leading, spacing: 0) {
            Rectangle()
                .fill(Tokens.lineSoft)
                .frame(height: 1)
                .padding(.vertical, 12)
            Text("Linked References · \(total)")
                .font(.label(sectionSize))
                .foregroundStyle(Tokens.muted)
                .padding(.horizontal, 20)
                .padding(.bottom, 8)
            ForEach(references, id: \.pageTitle) { group in
                Button {
                    if let first = group.hits.first {
                        open(Route(pageID: first.pageId, blockID: first.blockId))
                    }
                } label: {
                    HStack {
                        Text(group.pageTitle)
                            .font(.system(size: childSize, weight: .medium))
                            .foregroundStyle(Tokens.ink)
                        Spacer()
                        Text("\(group.hits.count)")
                            .font(.label(labelSize))
                            .foregroundStyle(Tokens.muted)
                    }
                    .padding(.horizontal, 20)
                    .frame(minHeight: 44)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                ForEach(group.hits, id: \.blockId) { hit in
                    HStack(alignment: .top, spacing: 8) {
                        Circle()
                            .fill(Tokens.bullet)
                            .frame(width: 5, height: 5)
                            .frame(width: Metrics.childBulletColumn, height: lineHeight, alignment: .leading)
                        Text(hit.text)
                            .font(.system(size: childSize))
                            .foregroundStyle(Tokens.ink3)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .padding(.leading, 32)
                    .padding(.trailing, 16)
                    .padding(.vertical, 2)
                    .contentShape(Rectangle())
                    .onTapGesture {
                        // The tapped reference, not the first one of its page (AC-7.2).
                        open(Route(pageID: hit.pageId, blockID: hit.blockId))
                    }
                }
            }
        }
    }
}

/// Find (ios-prd IO-8): one field, pages first and then blocks, each with its page title.
struct SearchScreen: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    var open: (String) -> Void

    @State private var query = ""
    @State private var results = SearchResults(pages: [], blocks: [])
    @State private var task: Task<Void, Never>?
    @FocusState private var fieldFocused: Bool

    @ScaledMetric(relativeTo: .body) private var textSize: CGFloat = 16
    @ScaledMetric(relativeTo: .caption) private var noteSize: CGFloat = 12

    private var trimmed: String { query.trimmingCharacters(in: .whitespaces) }

    var body: some View {
        NavigationStack {
            ScrollView {
                LazyVStack(spacing: 0) {
                    if trimmed.isEmpty {
                        // AC-8.2: Today, Yesterday and the pages changed most recently.
                        Text("Recent")
                            .font(.label(noteSize))
                            .foregroundStyle(Tokens.muted)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 20)
                            .padding(.vertical, 6)
                        ForEach(days, id: \.id) { page in
                            row(title: page.title, note: "Day", page: page.id)
                        }
                        ForEach(recent, id: \.id) { page in
                            row(title: page.title, note: page.date == nil ? "Page" : "Day", page: page.id)
                        }
                    } else {
                        ForEach(results.pages, id: \.id) { page in
                            row(title: page.title, note: "Page", page: page.id)
                        }
                        ForEach(results.blocks, id: \.blockId) { hit in
                            row(title: hit.text, note: hit.pageTitle, page: hit.pageId)
                        }
                        if results.pages.isEmpty && results.blocks.isEmpty {
                            Text("No page with this title.")
                                .font(.system(size: textSize))
                                .foregroundStyle(Tokens.muted)
                                .padding(.vertical, 24)
                        }
                    }
                }
            }
            .background(Tokens.paper)
            .navigationTitle("Find")
            .toolbarTitleDisplayMode(.inlineLarge)
            .searchable(
                text: $query,
                placement: .navigationBarDrawer(displayMode: .always),
                prompt: "Find a page or words"
            )
            .searchFocused($fieldFocused)
            .autocorrectionDisabled()
            .onChange(of: query) { _, _ in search() }
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(role: .close) { dismiss() }
                }
            }
        }
        .presentationDragIndicator(.visible)
        .onAppear {
            fieldFocused = true
            Task { await model.refreshRecents() }
        }
        .onDisappear { task?.cancel() }
    }

    /// Today and yesterday, when they have a page (AC-8.2).
    private var days: [PageRef] {
        var out: [PageRef] = []
        if let today = model.today, today.exists {
            out.append(PageRef(id: today.id, title: today.title, date: today.date))
        }
        if let yesterday = model.earlier.first, yesterday.exists {
            out.append(PageRef(id: yesterday.id, title: yesterday.title, date: yesterday.date))
        }
        return out
    }

    private var recent: [PageRef] {
        let shown = Set(days.map(\.id))
        return model.recentPages.filter { !shown.contains($0.id) }.prefix(8).map { $0 }
    }

    private func row(title: String, note: String, page: String) -> some View {
        Button {
            open(page)
        } label: {
            HStack(spacing: 8) {
                Text(title)
                    .font(.system(size: textSize))
                    .foregroundStyle(Tokens.ink)
                    .lineLimit(2)
                Spacer(minLength: 8)
                Text(note)
                    .font(.label(noteSize))
                    .foregroundStyle(Tokens.muted)
                    .lineLimit(1)
            }
            .padding(.horizontal, 20)
            .frame(minHeight: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func search() {
        task?.cancel()
        guard !trimmed.isEmpty else {
            results = SearchResults(pages: [], blocks: [])
            return
        }
        task = Task {
            try? await Task.sleep(for: .milliseconds(120))
            guard !Task.isCancelled else { return }
            if let found = await model.search(trimmed) {
                results = found
            }
        }
    }
}
