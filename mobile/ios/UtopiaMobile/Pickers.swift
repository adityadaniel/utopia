// The page picker and the day picker: they take the place of the keyboard below the compose
// box, which stays visible above them (ios-prd §4.6).

import SwiftUI

/// The page picker (AC-4.5, AC-4.6, AC-4.7): recent pages, a search by title or alias, and
/// "Create" for a title that no page has.
struct LinkPickerPanel: View {
    @Environment(AppModel.self) private var model
    @State private var query = ""
    @State private var results: [PageRef] = []
    @State private var task: Task<Void, Never>?
    @FocusState private var searchFocused: Bool

    @ScaledMetric(relativeTo: .body) private var textSize: CGFloat = 16
    @ScaledMetric(relativeTo: .body) private var fieldHeight: CGFloat = 44
    @ScaledMetric(relativeTo: .caption) private var noteSize: CGFloat = 12

    private var trimmed: String { query.trimmingCharacters(in: .whitespaces) }

    private var rows: [PageRef] {
        trimmed.isEmpty ? model.recentPages : results
    }

    private var offersCreate: Bool {
        !trimmed.isEmpty && !rows.contains { $0.title.caseInsensitiveCompare(trimmed) == .orderedSame }
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                TextField("Search or create a page", text: $query)
                    .font(.system(size: textSize))
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .focused($searchFocused)
                    .padding(.horizontal, 12)
                    .frame(height: fieldHeight)
                    .background(
                        RoundedRectangle(cornerRadius: 8, style: .continuous)
                            .fill(Tokens.paper)
                            .overlay(
                                RoundedRectangle(cornerRadius: 8, style: .continuous)
                                    .strokeBorder(Tokens.lineStrong, lineWidth: 1)
                            )
                    )
                    .onChange(of: query) { _, _ in search() }
                Button("Close") { model.closePicker() }
                    .font(.system(size: textSize))
                    .foregroundStyle(Tokens.link)
                    .frame(minWidth: 44, minHeight: 44)
                    .accessibilityLabel("Close")
            }
            .padding(.horizontal, 12)
            .padding(.top, 10)
            .padding(.bottom, 6)

            ScrollView {
                LazyVStack(spacing: 0) {
                    ForEach(rows, id: \.id) { page in
                        Button {
                            model.insertLink(title: page.title)
                        } label: {
                            HStack(spacing: 8) {
                                Text(page.title)
                                    .font(.system(size: textSize))
                                    .foregroundStyle(Tokens.ink)
                                    .lineLimit(1)
                                Spacer(minLength: 8)
                                Text("Page")
                                    .font(.label(noteSize))
                                    .foregroundStyle(Tokens.muted)
                            }
                            .padding(.horizontal, 16)
                            .frame(height: 44)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                    if offersCreate {
                        Button {
                            model.insertLink(title: trimmed)
                        } label: {
                            HStack(spacing: 8) {
                                Text("Create “\(trimmed)”")
                                    .font(.system(size: textSize))
                                    .foregroundStyle(Tokens.ink)
                                    .lineLimit(1)
                                Spacer(minLength: 8)
                                Text("New page")
                                    .font(.label(noteSize))
                                    .foregroundStyle(Tokens.muted)
                            }
                            .padding(.horizontal, 16)
                            .frame(height: 44)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                    if rows.isEmpty && !offersCreate {
                        Text("No page with this title.")
                            .font(.system(size: noteSize + 2))
                            .foregroundStyle(Tokens.muted)
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 24)
                    }
                }
            }
            .scrollDismissesKeyboard(.never)
        }
        .frame(maxWidth: .infinity)
        // The panel takes the keyboard's place with the keyboard down, so the day log stays in
        // view (§4.6); a tap on the search field brings the keyboard and the panel shrinks to
        // whatever is left.
        .frame(maxHeight: 320)
        .background(Tokens.well)
        .overlay(alignment: .top) {
            Rectangle().fill(Tokens.line).frame(height: 1)
        }
        .onAppear {
            results = model.recentPages
        }
        .onDisappear { task?.cancel() }
    }

    private func search() {
        task?.cancel()
        let words = trimmed
        guard !words.isEmpty else {
            results = model.recentPages
            return
        }
        task = Task {
            try? await Task.sleep(for: .milliseconds(120))
            guard !Task.isCancelled else { return }
            if let found = await model.search(words) {
                results = found.pages + found.blocks.map {
                    PageRef(id: $0.pageId, title: $0.pageTitle, date: nil)
                }
            }
        }
    }
}

/// The day picker (AC-4.9): Today, Tomorrow and the system date picker.
struct DayPickerPanel: View {
    @Environment(AppModel.self) private var model
    @State private var picking = false
    @State private var date = Date()

    @ScaledMetric(relativeTo: .body) private var textSize: CGFloat = 16

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Spacer()
                Button("Close") { model.closePicker() }
                    .font(.system(size: textSize))
                    .foregroundStyle(Tokens.link)
                    .frame(minWidth: 44, minHeight: 44)
                    .accessibilityLabel("Close")
            }
            .padding(.horizontal, 12)
            .padding(.top, 6)

            ScrollView {
                VStack(spacing: 0) {
                    row("Today") { insert(offsetDays: 0) }
                    row("Tomorrow") { insert(offsetDays: 1) }
                    row("Pick a date…") { picking.toggle() }
                    if picking {
                        DatePicker(
                            "Date",
                            selection: $date,
                            displayedComponents: .date
                        )
                        .datePickerStyle(.graphical)
                        .padding(.horizontal, 12)
                        .onChange(of: date) { _, newValue in
                            Task {
                                let title = await model.title(forISODate: model.sqlDate(newValue))
                                model.insertLink(title: title)
                            }
                        }
                    }
                }
            }
        }
        .frame(maxWidth: .infinity)
        .frame(maxHeight: 320)
        .background(Tokens.well)
        .overlay(alignment: .top) {
            Rectangle().fill(Tokens.line).frame(height: 1)
        }
    }

    private func row(_ label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack {
                Text(label)
                    .font(.system(size: textSize))
                    .foregroundStyle(Tokens.ink)
                Spacer()
            }
            .padding(.horizontal, 16)
            .frame(height: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func insert(offsetDays: Int) {
        Task {
            let iso = model.sqlDate(Calendar.current.date(byAdding: .day, value: offsetDays, to: Date()) ?? Date())
            let title = await model.title(forISODate: iso)
            model.insertLink(title: title)
        }
    }
}
