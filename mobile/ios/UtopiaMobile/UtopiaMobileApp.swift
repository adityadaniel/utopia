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
