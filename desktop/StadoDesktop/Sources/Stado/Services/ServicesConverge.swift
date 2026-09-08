import SwiftUI
import WisentDesignSystem

/// The convergence request, and the receipt it leaves behind.
///
/// `convergenceReceiptPanel` is internal rather than private because `alarms`
/// sits in `ServicesAlarms.swift`, and `prepareConvergence` and
/// `convergeSheet` because `body` sits in `ServicesView.swift`: Swift scopes
/// `private` to one file.
extension ServicesView {
    func convergenceReceiptPanel(_ receipt: ServiceConvergeReceipt) -> some View {
        WisentSectionBox(
            title: "Convergence receipt",
            detail: "The complete report and product exit code returned by the API. Refreshing service state does not replace them.",
            trailing: "exit \(receipt.exitCode)"
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                Text(verbatim: StadoCLI.commandLine(receipt.arguments))
                    .font(WisentTypeScale.identifierSmall())
                    .foregroundStyle(WisentDesign.ink)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                Text(verbatim: receipt.json)
                    .font(WisentTypeScale.identifierSmall())
                    .foregroundStyle(WisentDesign.secondary)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                WisentActionButton(
                    action: WisentAction("Dismiss receipt", kind: .plain) {
                        fleetStore.clearConvergenceReceipt()
                    }
                )
            }
        }
    }


    private var availableConvergeBinaries: [String] {
        Array(Set(store.units.filter { $0.host == convergeHost }.map(\.unit.binary).filter { !$0.isEmpty }))
            .sorted()
    }

    func prepareConvergence() {
        let selectedUnit = store.units.first { $0.id == selection }
        let selectedFleetHost = fleetRows.first { $0.id == selection }.flatMap { row -> String? in
            switch row {
            case let .service(entry): entry.host
            case let .unavailable(host, _): host
            }
        }
        convergeHost = selectedUnit?.host ?? selectedFleetHost ?? hosts.first ?? ""
        convergeBinary = selectedUnit?.unit.binary ?? ""
        showsConverge = true
    }

    var convergeSheet: some View {
        let binary = convergeBinary.isEmpty ? nil : convergeBinary
        let arguments = FleetServicesStore.convergeApplyArguments(host: convergeHost, binary: binary)
        return VStack(alignment: .leading, spacing: WisentDesign.Space.x6) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                Text("Converge declared service binaries")
                    .font(WisentTypeScale.screenTitle())
                    .foregroundStyle(WisentDesign.ink)
                Text("Choose the registry host and keep the selected binary, or apply every binary that host declares.")
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.secondary)
            }
            WisentSectionBox(
                title: "Authenticated convergence request",
                detail: "Stado owns delivery, refusal and final verification. Desktop sends one apply request to the configured product API; the command below is equivalent information only."
            ) {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                    Picker("Host", selection: $convergeHost) {
                        ForEach(hosts, id: \.self) { Text($0).tag($0) }
                    }
                    .pickerStyle(.menu)
                    Picker("Binary", selection: $convergeBinary) {
                        Text("All declared binaries").tag("")
                        ForEach(availableConvergeBinaries, id: \.self) { Text($0).tag($0) }
                    }
                    .pickerStyle(.menu)
                    Text("Equivalent CLI command")
                        .font(WisentTypeScale.eyebrow())
                        .foregroundStyle(WisentDesign.muted)
                    Text(verbatim: StadoCLI.commandLine(arguments))
                        .font(WisentTypeScale.identifier())
                        .foregroundStyle(WisentDesign.ink)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            Text("Convergence may replace installed programs and restart their services. A newer host version is refused rather than downgraded. The receipt remains visible even when only part of the request can be delivered.")
                .font(WisentTypeScale.body())
                .foregroundStyle(WisentDesign.secondary)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: WisentDesign.Space.x2) {
                WisentActionButton(action: WisentAction("Cancel", kind: .primary) { showsConverge = false })
                Spacer(minLength:
                    0
                )
                WisentActionButton(
                    action: WisentAction(
                        "Apply convergence",
                        symbol: "arrow.triangle.2.circlepath",
                        kind: .destructive,
                        isEnabled: !convergeHost.isEmpty && !fleetStore.mutation.isWorking
                    ) {
                        let host = convergeHost
                        let binary = convergeBinary.isEmpty ? nil : convergeBinary
                        showsConverge = false
                        Task {
                            await fleetStore.converge(host: host, binary: binary)
                            await store.refresh(hosts: hosts)
                        }
                    }
                )
            }
        }
        .padding(WisentDesign.Space.x6)
        .frame(minWidth:
            680
        )
        .onChange(of: convergeHost) { _, _ in
            if !convergeBinary.isEmpty, !availableConvergeBinaries.contains(convergeBinary) {
                convergeBinary = ""
            }
        }
    }
}
