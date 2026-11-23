// First run (ios-prd IO-10): start a new graph, or connect to the one on the Mac. The relay
// token and the passphrase are secure fields and never shown again (AC-10.4).

import SwiftUI

struct SetupScreen: View {
    @Environment(AppModel.self) private var model
    @State private var connecting = false
    @State private var relayURL = ""
    @State private var graphID = ""
    @State private var token = ""
    @State private var passphrase = ""
    @State private var busy = false

    @ScaledMetric(relativeTo: .body) private var textSize: CGFloat = 16

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    Text("Add thoughts to today's note. Read your pages.")
                        .font(.system(size: textSize))
                        .foregroundStyle(Tokens.muted)

                    Button {
                        busy = true
                        Task {
                            await model.startNewGraph()
                            busy = false
                        }
                    } label: {
                        Text("Start a new graph")
                            .font(.system(size: textSize, weight: .medium))
                            .frame(maxWidth: .infinity, minHeight: 44)
                    }
                    .buttonStyle(.glassProminent)
                    .tint(Tokens.link)
                    .disabled(busy)

                    Button {
                        connecting = true
                    } label: {
                        Text("Connect to my graph")
                            .font(.system(size: textSize, weight: .medium))
                            .frame(maxWidth: .infinity, minHeight: 44)
                    }
                    .buttonStyle(.glass)

                    if let message = model.errorMessage, !connecting {
                        Text(message)
                            .font(.system(size: textSize - 2))
                            .foregroundStyle(Tokens.danger)
                    }
                }
                .padding(.horizontal, 20)
                .padding(.top, 8)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Tokens.paper)
            .navigationTitle("Utopia")
            .toolbarTitleDisplayMode(.inlineLarge)
            .navigationDestination(isPresented: $connecting) {
                connectForm
            }
        }
    }

    /// "Connect to my graph" (AC-10.2): a pushed screen, so the system bar gives Back, and
    /// Connect is the bar's confirmation action.
    private var connectForm: some View {
        Form {
            Section("Relay") {
                TextField("https://relay.example.com", text: $relayURL)
                    .textContentType(.URL)
                    .keyboardType(.URL)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                TextField("Graph id", text: $graphID)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
            }
            Section("Secrets") {
                SecureField("Token", text: $token)
                SecureField("Passphrase", text: $passphrase)
            }
            if let message = model.errorMessage {
                Section {
                    Text(message)
                        .foregroundStyle(Tokens.danger)
                }
            }
        }
        .navigationTitle("Connect")
        .toolbarTitleDisplayMode(.inlineLarge)
        .toolbar {
            ToolbarItem(placement: .confirmationAction) {
                if busy {
                    ProgressView()
                } else {
                    Button("Connect") {
                        busy = true
                        Task {
                            await model.connect(
                                relayURL: relayURL,
                                graphID: graphID,
                                token: token,
                                passphrase: passphrase,
                                databasePassphrase: nil
                            )
                            busy = false
                        }
                    }
                    .disabled(!canConnect)
                }
            }
        }
    }

    private var canConnect: Bool {
        !relayURL.trimmingCharacters(in: .whitespaces).isEmpty
            && !graphID.trimmingCharacters(in: .whitespaces).isEmpty
            && !token.isEmpty
            && !passphrase.isEmpty
    }
}

/// A graph encrypted at rest asks for its passphrase before it opens.
struct LockedScreen: View {
    @Environment(AppModel.self) private var model
    @State private var passphrase = ""
    @FocusState private var focused: Bool

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    SecureField("Passphrase", text: $passphrase)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .focused($focused)
                        .onSubmit(unlock)
                } footer: {
                    Text("Type its passphrase to open it.")
                }
                if let message = model.errorMessage {
                    Section {
                        Text(message)
                            .foregroundStyle(Tokens.danger)
                    }
                }
            }
            .navigationTitle("This graph is locked")
            .toolbarTitleDisplayMode(.inlineLarge)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Unlock", action: unlock)
                        .disabled(passphrase.isEmpty)
                }
            }
            .onAppear { focused = true }
        }
    }

    private func unlock() {
        guard !passphrase.isEmpty else { return }
        Task { await model.unlock(passphrase: passphrase) }
    }
}

/// Settings → Sync (IO-9): the relay of this phone. Blank secret fields keep the saved values
/// (AC-10.4).
struct SyncSettingsScreen: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    @State private var relayURL = ""
    @State private var graphID = ""
    @State private var token = ""
    @State private var passphrase = ""

    @ScaledMetric(relativeTo: .body) private var textSize: CGFloat = 16

    var body: some View {
        NavigationStack {
            Form {
                Section("Relay") {
                    TextField("https://relay.example.com", text: $relayURL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    TextField("Graph id", text: $graphID)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                }
                Section("Secrets") {
                    SecureField("Token", text: $token)
                    SecureField("Passphrase", text: $passphrase)
                }
                Section {
                    Button(model.syncState.text) {
                        model.setSync(relayURL: relayURL, graphID: graphID, token: token, passphrase: passphrase)
                        Task { await model.syncNow() }
                    }
                    .disabled(relayURL.isEmpty || graphID.isEmpty)
                }
            }
            .navigationTitle("Sync")
            .toolbarTitleDisplayMode(.inlineLarge)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(role: .close) { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(role: .confirm) {
                        // Whatever is in the fields is what is kept, including an empty relay
                        // turned off on purpose.
                        model.setSync(relayURL: relayURL, graphID: graphID, token: token, passphrase: passphrase)
                        dismiss()
                    }
                }
            }
            .onAppear {
                let settings = SyncSettings.load()
                relayURL = settings.relayURL
                graphID = settings.graphID
                token = Keychain.get(Keychain.relayToken) ?? ""
                passphrase = Keychain.get(Keychain.e2ePassphrase) ?? ""
            }
        }
    }
}
