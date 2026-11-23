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

    @ScaledMetric(relativeTo: .title) private var titleSize: CGFloat = 28
    @ScaledMetric(relativeTo: .body) private var textSize: CGFloat = 16

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Text("Utopia")
                    .font(.system(size: titleSize, weight: .semibold))
                    .foregroundStyle(Tokens.ink)
                    .padding(.top, 32)
                Text("Add thoughts to today's note. Read your pages.")
                    .font(.system(size: textSize))
                    .foregroundStyle(Tokens.muted)

                if connecting {
                    form
                } else {
                    Button {
                        busy = true
                        Task {
                            await model.startNewGraph()
                            busy = false
                        }
                    } label: {
                        Text("Start a new graph")
                            .font(.system(size: textSize, weight: .medium))
                            .foregroundStyle(Tokens.paper)
                            .frame(maxWidth: .infinity)
                            .frame(height: 48)
                            .background(RoundedRectangle(cornerRadius: 10).fill(Tokens.link))
                    }
                    .buttonStyle(.plain)
                    .disabled(busy)

                    Button {
                        connecting = true
                    } label: {
                        Text("Connect to my graph")
                            .font(.system(size: textSize, weight: .medium))
                            .foregroundStyle(Tokens.ink)
                            .frame(maxWidth: .infinity)
                            .frame(height: 48)
                            .background(
                                RoundedRectangle(cornerRadius: 10)
                                    .fill(Tokens.paper)
                                    .overlay(
                                        RoundedRectangle(cornerRadius: 10)
                                            .strokeBorder(Tokens.lineStrong, lineWidth: 1)
                                    )
                            )
                    }
                    .buttonStyle(.plain)
                }

                if let message = model.errorMessage {
                    Text(message)
                        .font(.system(size: textSize - 2))
                        .foregroundStyle(Tokens.danger)
                }
            }
            .padding(.horizontal, 24)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Tokens.paper)
    }

    private var form: some View {
        VStack(alignment: .leading, spacing: 12) {
            field("Relay URL", text: $relayURL, secure: false)
            field("Graph id", text: $graphID, secure: false)
            field("Token", text: $token, secure: true)
            field("Passphrase", text: $passphrase, secure: true)
            Button {
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
            } label: {
                Text(busy ? "Connecting…" : "Connect")
                    .font(.system(size: textSize, weight: .medium))
                    .foregroundStyle(Tokens.paper)
                    .frame(maxWidth: .infinity)
                    .frame(height: 48)
                    .background(RoundedRectangle(cornerRadius: 10).fill(canConnect ? Tokens.link : Tokens.disabled))
            }
            .buttonStyle(.plain)
            .disabled(!canConnect || busy)
            Button("Back") { connecting = false }
                .font(.system(size: textSize))
                .foregroundStyle(Tokens.muted)
                .frame(minHeight: 44)
        }
    }

    private var canConnect: Bool {
        !relayURL.trimmingCharacters(in: .whitespaces).isEmpty
            && !graphID.trimmingCharacters(in: .whitespaces).isEmpty
            && !token.isEmpty
            && !passphrase.isEmpty
    }

    @ViewBuilder
    private func field(_ label: String, text: Binding<String>, secure: Bool) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label)
                .font(.system(size: textSize - 3))
                .foregroundStyle(Tokens.muted)
            Group {
                if secure {
                    SecureField(label, text: text)
                } else {
                    TextField(label, text: text)
                }
            }
            .font(.system(size: textSize))
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .padding(.horizontal, 12)
            .frame(height: 44)
            .background(
                RoundedRectangle(cornerRadius: 8)
                    .fill(Tokens.well)
                    .overlay(
                        RoundedRectangle(cornerRadius: 8)
                            .strokeBorder(Tokens.lineStrong, lineWidth: 1)
                    )
            )
        }
    }
}

/// A graph encrypted at rest asks for its passphrase before it opens.
struct LockedScreen: View {
    @Environment(AppModel.self) private var model
    @State private var passphrase = ""

    @ScaledMetric(relativeTo: .body) private var textSize: CGFloat = 16

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("This graph is locked")
                .font(.system(size: textSize + 6, weight: .semibold))
                .foregroundStyle(Tokens.ink)
            Text("Type its passphrase to open it.")
                .font(.system(size: textSize))
                .foregroundStyle(Tokens.muted)
            SecureField("Passphrase", text: $passphrase)
                .font(.system(size: textSize))
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .padding(.horizontal, 12)
                .frame(height: 44)
                .background(
                    RoundedRectangle(cornerRadius: 8)
                        .fill(Tokens.well)
                        .overlay(
                            RoundedRectangle(cornerRadius: 8)
                                .strokeBorder(Tokens.lineStrong, lineWidth: 1)
                        )
                )
            Button("Unlock") {
                Task { await model.unlock(passphrase: passphrase) }
            }
            .font(.system(size: textSize, weight: .medium))
            .foregroundStyle(Tokens.paper)
            .frame(maxWidth: .infinity)
            .frame(height: 48)
            .background(RoundedRectangle(cornerRadius: 10).fill(Tokens.link))
            if let message = model.errorMessage {
                Text(message)
                    .font(.system(size: textSize - 2))
                    .foregroundStyle(Tokens.danger)
            }
        }
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
        .background(Tokens.paper)
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
                    Button("Done") {
                        // Whatever is in the fields is what is kept, including an empty relay
                        // turned off on purpose.
                        model.setSync(relayURL: relayURL, graphID: graphID, token: token, passphrase: passphrase)
                        dismiss()
                    }
                }
            }
            .navigationTitle("Sync")
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
