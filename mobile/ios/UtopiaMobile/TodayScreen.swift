// The Today screen (ios-prd §4.3): the header, the day log, the chips, the compose box and,
// in the keyboard's place, a picker. The screen itself does not scroll; only the log and,
// when it is long, the lines of the draft do.

import SwiftUI

/// Where a page screen was opened from: a page, and optionally the block to show (AC-7.2).
struct Route: Hashable {
    var pageID: String
    var blockID: String?
}

struct TodayScreen: View {
    @Environment(AppModel.self) private var model
    @State private var path: [Route] = []
    @State private var searching = false
    @State private var settingsOpen = false

    @ScaledMetric(relativeTo: .caption) private var statusSize: CGFloat = 13

    var body: some View {
        NavigationStack(path: $path) {
            VStack(spacing: 0) {
                // The log is the one part that gives way: the compose box, the chips and a
                // picker keep their height (§4.3, §4.6).
                DayLog { page in path.append(Route(pageID: page)) }
                    .frame(maxHeight: .infinity)
                    .layoutPriority(0)
                bottom
                    .layoutPriority(1)
            }
            .background(Tokens.paper)
            // The header of §4.3 is the system navigation bar: the day's title, the weekday
            // under it, and the sync status and Find in the bar's trailing edge.
            .navigationTitle(model.today?.title ?? "Today")
            .navigationSubtitle(weekday)
            .toolbarTitleDisplayMode(.inlineLarge)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        settingsOpen = true
                    } label: {
                        SyncStatusLine(state: model.syncState, size: statusSize)
                    }
                    .accessibilityLabel("Sync: \(model.syncState.text)")
                }
                ToolbarSpacer(.fixed, placement: .topBarTrailing)
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        searching = true
                    } label: {
                        Label("Find", systemImage: "magnifyingglass")
                    }
                }
            }
            .navigationDestination(for: Route.self) { route in
                PageScreen(route: route) { target in
                    path.append(target)
                }
            }
        }
        .sheet(isPresented: $searching) {
            SearchScreen { page in
                searching = false
                path.append(Route(pageID: page))
            }
        }
        .sheet(isPresented: $settingsOpen) {
            SyncSettingsScreen()
        }
        .task {
            if model.phase == .launching {
                await model.launch()
            }
            await model.refreshRecents()
        }
    }

    private var weekday: String {
        Date().formatted(.dateTime.weekday(.wide))
    }

    // MARK: - The bottom area: chips, the box, and a picker in the keyboard's place

    private var bottom: some View {
        VStack(spacing: 0) {
            if let message = model.errorMessage {
                Text(message)
                    .font(.system(size: statusSize))
                    .foregroundStyle(Tokens.danger)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 12)
                    .padding(.top, 6)
            }
            VStack(spacing: 8) {
                if !model.suggestions.isEmpty && model.picker == nil {
                    SuggestionRow()
                }
                ComposeBox()
            }
            .padding(.horizontal, 12)
            .padding(.top, 8)
            .padding(.bottom, 10)

            switch model.picker {
            case .link: LinkPickerPanel()
            case .date: DayPickerPanel()
            case nil: EmptyView()
            }
        }
        .background(Tokens.paper)
    }
}

/// The sync dot and its short text (§4.3). The dot is the one colour a reader reads at a
/// glance: `success` when in sync, `disabled` when offline.
struct SyncStatusLine: View {
    let state: SyncState
    let size: CGFloat

    var body: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(colour)
                .frame(width: 8, height: 8)
            Text(state.shortText)
                .font(.system(size: size))
                .foregroundStyle(Tokens.muted)
                .lineLimit(1)
        }
    }

    private var colour: Color {
        switch state {
        case .synced: Tokens.success
        case .syncing: Tokens.bullet
        case .offline, .off: Tokens.disabled
        case .needsAuth, .failed: Tokens.danger
        case .unsupported, .blocked: Tokens.warning
        }
    }
}

#if DEBUG
#Preview("Today") {
    TodayScreen()
        .environment(AppModel.preview())
}

#Preview("Today · Dark") {
    TodayScreen()
        .environment(AppModel.preview())
        .preferredColorScheme(.dark)
}

#Preview("Today · Empty day") {
    TodayScreen()
        .environment(AppModel.preview(today: PreviewData.emptyToday, syncState: .off, draft: PreviewData.emptyDraft))
}

#Preview("Today · Offline with an error") {
    TodayScreen()
        .environment(AppModel.preview(
            syncState: .offline(queued: 3),
            errorMessage: "Sync: the relay did not answer."
        ))
}

#Preview("Today · Suggestions") {
    TodayScreen()
        .environment(AppModel.preview(
            draft: Draft(lines: [DraftLine(text: "Meeting with Dana about the budget")]),
            suggestions: PreviewData.suggestions
        ))
}

#Preview("Today · Link picker") {
    TodayScreen()
        .environment(AppModel.preview(picker: .link))
}

#Preview("Today · Day picker") {
    TodayScreen()
        .environment(AppModel.preview(picker: .date))
}

#Preview("Sync status · Every state") {
    let states: [SyncState] = [
        .off, .syncing, .synced, .offline(queued: 0), .offline(queued: 2),
        .needsAuth, .unsupported(version: 3), .failed("Timed out"),
    ]
    VStack(alignment: .leading, spacing: 12) {
        ForEach(states, id: \.text) { state in
            SyncStatusLine(state: state, size: 13)
        }
    }
    .padding()
}
#endif
