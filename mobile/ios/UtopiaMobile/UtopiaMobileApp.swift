// Utopia for iPhone: one graph, one screen that adds to today's note, and the pages behind
// its links (ios-prd §4, C9).

import SwiftUI

@main
struct UtopiaMobileApp: App {
    @State private var model = AppModel()

    var body: some Scene {
        WindowGroup {
            RootScreen()
                .environment(model)
                .tint(Tokens.link)
        }
    }
}

struct RootScreen: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Group {
            switch model.phase {
            case .launching:
                ProgressView()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(Tokens.paper)
            case .setup:
                SetupScreen()
            case .locked:
                LockedScreen()
            case .ready:
                TodayScreen()
            }
        }
        .task {
            if model.phase == .launching {
                await model.launch()
            }
        }
    }
}

#if DEBUG
#Preview("Root · Launching") {
    RootScreen()
        .environment(AppModel.preview(phase: .launching))
}

#Preview("Root · Setup") {
    RootScreen()
        .environment(AppModel.preview(phase: .setup))
}

#Preview("Root · Locked") {
    RootScreen()
        .environment(AppModel.preview(phase: .locked))
}

#Preview("Root · Ready") {
    RootScreen()
        .environment(AppModel.preview())
}
#endif
