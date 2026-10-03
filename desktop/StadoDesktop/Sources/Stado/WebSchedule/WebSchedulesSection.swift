import SwiftUI
import WisentDesignSystem

/// Web hosting → Scheduled requests: the same declare, withdraw and list as
/// `stado web schedule set|remove|list`, with Stado's receipts and refusals
/// shown as Stado wrote them. Declaring a schedule sends nothing; the fleet
/// schedules are created by `stado web route` at the product's cutover.
struct WebSchedulesSection: View {
    @ObservedObject var store: WebScheduleStore
    @State private var draft = WebScheduleDraft()

    var body: some View {
        WisentSectionBox(
            title: "Scheduled requests",
            detail: "Requests a web product's unit is sent on a cron. `stado web route` starts sending them at the cutover; `stado web remove` stops them."
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                WisentMutationBar(outcome: store.mutation) { store.clearMutation() }
                if let problem = store.readFailure {
                    Text(problem)
                        .font(WisentTypeScale.caption())
                        .textSelection(.enabled)
                }
                ForEach(store.rows) { row in
                    rowView(row)
                }
                if store.rows.isEmpty, store.readFailure == nil, store.result != nil {
                    Text("No web product declares a scheduled request.")
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentDesign.secondary)
                }
                Divider()
                form
                if let result = store.result {
                    DisclosureGroup("Command output") {
                        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                            Text(StadoCLI.commandLine(result.arguments))
                            Text("Exit code: \(result.exitCode.map(String.init) ?? "not reported")")
                            Text(result.standardOutput)
                            Text(result.standardError)
                        }
                        .font(WisentTypeScale.identifierSmall())
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
            }
        }
    }

    private func rowView(_ row: WebScheduleRow) -> some View {
        HStack(alignment: .top, spacing: WisentDesign.Space.x3) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                Text("\(row.product)/\(row.schedule): \(row.method) \(row.path)")
                    .font(WisentTypeScale.identifierSmall())
                Text("\(row.cron) (\(row.tz))")
                    .font(WisentTypeScale.caption())
                if let fleet = row.fleetSchedule {
                    Text("\(fleet.id) \(fleet.enabled ? "enabled" : "paused"), next \(fleet.nextDueAt), last job \(fleet.lastJobId.isEmpty ? "none" : fleet.lastJobId)")
                        .font(WisentTypeScale.caption())
                        .textSelection(.enabled)
                } else {
                    Text("Not sent yet: the product has not been routed.")
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentDesign.secondary)
                }
            }
            Spacer()
            WisentActionButton(
                action: WisentAction(
                    "Withdraw",
                    symbol: "minus.circle",
                    isEnabled: !store.mutation.isWorking
                ) {
                    Task { await store.remove(product: row.product, schedule: row.schedule) }
                }
            )
            .accessibilityIdentifier("web-schedule-remove-\(row.id)")
        }
    }

    private var form: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            Text("Declare or change a scheduled request")
                .font(WisentTypeScale.caption())
            HStack {
                TextField("Web product", text: $draft.product)
                TextField("Schedule name", text: $draft.name)
            }
            HStack {
                TextField("Method, e.g. POST", text: $draft.method)
                TextField("Path, e.g. /api/cron/report", text: $draft.path)
            }
            HStack {
                TextField("Cron, 5 fields", text: $draft.cron)
                TextField("IANA time zone", text: $draft.tz)
            }
            HStack {
                TextField("Secret header (optional)", text: $draft.secretHeader)
                TextField("Scheme (optional)", text: $draft.secretScheme)
                TextField("role#field (optional)", text: $draft.secret)
            }
            WisentActionButton(
                action: WisentAction(
                    store.mutation.isWorking ? "Writing…" : "Declare schedule",
                    symbol: "calendar.badge.plus",
                    isEnabled: !store.mutation.isWorking
                ) {
                    let submitted = draft
                    Task { await store.set(submitted) }
                }
            )
            .accessibilityIdentifier("web-schedule-set")
        }
        .textFieldStyle(.roundedBorder)
    }
}
