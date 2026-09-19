import Foundation
import SwiftUI
import WisentDesignSystem

/// Start one session the fleet keeps running without an operator attached.
///
/// The sheet asks for exactly what `stado workload start` accepts, because a
/// detached session is granted nothing it was not given here: it may write
/// files or run commands only when those switches are on.
struct WorkloadStartView: View {
    let kind: String
    let target: String
    @ObservedObject var store: WorkloadStore
    @ObservedObject var fleet: FleetControlStore
    @Environment(\.dismiss) private var dismiss

    @State private var workspace = "__home__"
    @State private var task = ""
    @State private var model = ""
    @State private var maxSteps = ""
    @State private var allowWrite = false
    @State private var allowCommand = false

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            Text("Start \(kind) detached")
                .font(WisentTypeScale.panelTitle())
                .foregroundStyle(WisentDesign.ink)
            Text("Placed on \(target) through the selected Stado API. The session survives this window, this application and this machine; follow it afterwards in Detached sessions.")
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.muted)
            TextField("Workspace on the host (__home__ for its home directory)", text: $workspace)
            TextField("Task the session is started for", text: $task, axis: .vertical)
            TextField("Model route (optional)", text: $model)
            TextField("Maximum steps (optional)", text: $maxSteps)
            Toggle("May write files in its workspace", isOn: $allowWrite)
            Toggle("May run commands in its workspace", isOn: $allowCommand)
            HStack {
                Button("Cancel") { dismiss() }
                Spacer()
                Button("Start session") {
                    Task {
                        await store.startSession(
                            DetachedSessionRequest(kind: kind, workspace: workspace, task: task,
                                model: model, maxSteps: maxSteps, allowWrite: allowWrite,
                                allowCommand: allowCommand),
                            target: target, fleet: fleet)
                        dismiss()
                    }
                }
                .disabled(store.isLoading
                    || task.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    || workspace.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
        .padding(WisentDesign.Space.x3)
    }
}

/// The sessions the fleet is holding right now, with the two actions an
/// operator needs on one: read it, or stop it.
struct DetachedSessionList: View {
    @ObservedObject var store: WorkloadStore
    @ObservedObject var fleet: FleetControlStore

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
            HStack {
                Text("Detached sessions")
                    .font(WisentTypeScale.identifier())
                    .foregroundStyle(WisentDesign.ink)
                Spacer()
                Button("Refresh") { Task { await store.loadSessions(fleet: fleet) } }
            }
            if let failure = store.sessionsFailure {
                WisentAlertPanel(tone: .danger, title: "Detached sessions unavailable", detail: failure)
            } else if store.sessions.isEmpty {
                WisentField(label: "Running detached", value: "None")
            } else {
                ForEach(store.sessions) { session in
                    HStack(alignment: .firstTextBaseline) {
                        VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                            Text("\(session.kind) · \(session.jobID)")
                                .font(WisentTypeScale.identifier())
                                .foregroundStyle(WisentDesign.ink)
                            Text("\(session.state) on \(session.host) · workspace \(session.workspace ?? "-") · started \(session.started ?? "-")")
                                .font(WisentTypeScale.caption())
                                .foregroundStyle(WisentDesign.muted)
                        }
                        Spacer(minLength: WisentDesign.Space.x2)
                        Button("Stop") {
                            Task { await store.cancelSession(jobID: session.jobID, fleet: fleet) }
                        }
                    }
                    .padding(.vertical, WisentDesign.Space.x1)
                }
            }
        }
    }
}
