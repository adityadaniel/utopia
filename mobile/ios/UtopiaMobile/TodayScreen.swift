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

    @ScaledMetric(relativeTo: .caption) private var labelSize: CGFloat = 12
    @ScaledMetric(relativeTo: .title2) private var titleSize: CGFloat = 22
    @ScaledMetric(relativeTo: .caption) private var statusSize: CGFloat = 13

    var body: some View {
        NavigationStack(path: $path) {
            VStack(spacing: 0) {
                header
                // The log is the one part that gives way: the compose box, the chips and a
                // picker keep their height (§4.3, §4.6).
                DayLog { page in path.append(Route(pageID: page)) }
                    .frame(maxHeight: .infinity)
                    .layoutPriority(0)
                bottom
                    .layoutPriority(1)
            }
            .background(Tokens.paper)
            .navigationDestination(for: Route.self) { route in
                PageScreen(route: route) { target in
                    path.append(target)
                }
            }
            .toolbar(.hidden, for: .navigationBar)
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

    // MARK: - Header (§4.3)

    private var header: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 10) {
                Text(weekday)
                    .font(.label(labelSize))
                    .foregroundStyle(Tokens.muted)
                Text("Today")
                    .font(.system(size: labelSize, weight: .medium))
                    .foregroundStyle(Tokens.link)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 2)
                    .background(Capsule().fill(Tokens.linkTint))
                Spacer(minLength: 8)
                Button {
                    searching = true
                } label: {
                    Image(systemName: "magnifyingglass")
                        .font(.system(size: 16))
                        .foregroundStyle(Tokens.ink3)
                        .frame(width: 44, height: 44)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Find")
            }
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(model.today?.title ?? "")
                    .font(.system(size: titleSize, weight: .semibold))
                    .foregroundStyle(Tokens.ink)
                Spacer(minLength: 8)
                Button {
                    settingsOpen = true
                } label: {
                    SyncStatusLine(state: model.syncState, size: statusSize)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Sync: \(model.syncState.text)")
            }
        }
        .padding(.horizontal, 20)
        .padding(.bottom, 8)
        .overlay(alignment: .bottom) {
            Rectangle().fill(Tokens.lineSoft).frame(height: 1)
        }
    }

    private var weekday: String {
        Date().formatted(.dateTime.weekday(.wide)).uppercased()
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
        case .unsupported: Tokens.warning
        }
    }
}
