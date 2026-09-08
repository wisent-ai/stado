import SwiftUI
import WisentDesignSystem

extension HostsView {
    @ViewBuilder
    func inspector(_ snapshot: DashboardSnapshot) -> some View {
        if let host = hosts(snapshot).first(where: { $0.id == selection }) {
            WisentInspector(
                eyebrow: "Host",
                title: host.displayName,
                badges: badges(for: host)
            ) {
                gateSection(for: host)
                if host.declared {
                    SpaceSection(host: host.targetName ?? host.displayName)
                }
                linkSection(for: host)
                CredentialsHostSection(
                    host: host.targetName ?? host.displayName,
                    store: vaultStore
                ) {
                    vaultBearerTarget = HostVaultBearerTarget(
                        host: host.targetName ?? host.displayName
                    )
                }
                RoutesSection(store: routesStore, selectedHost: host.targetName ?? host.displayName)
                tailscaleLogSection(for: host)
                HostReleaseSection(
                    store: store.hostReleaseStore,
                    host: host.targetName ?? host.displayName
                )


                appleChallengeSection(for: host)
                cargoInventorySection(for: host)
                RunnerSection(host: host, fleetStore: fleetStore)
                WorkloadSection(
                    target: host.targetName ?? host.displayName,
                    store: workloadStore
                )
                RepairSection(store: repairStore, host: host.targetName ?? host.displayName)
                ScratchSection(host: host.targetName ?? host.displayName)
                if host.status != .live {
                    WisentAlertPanel(
                        tone: tone(for: host.status),
                        title: host.status == .unavailable ? "No current capacity report" : "Capacity report is stale",
                        detail: host.availabilityReason
                    )
                }
                WisentField(label: "Consumer identity", value: host.consumerID ?? "Not reported")
                WisentField(label: "Kind", value: host.kind?.humanizedIdentifier ?? "Not reported")
                WisentField(label: "Role", value: host.role?.humanizedIdentifier ?? "Not reported")
                WisentField(label: "GPU", value: host.gpuType ?? "Not reported")
                WisentField(
                    label: "Hostnames",
                    value: host.hostnames.isEmpty ? "Not reported" : host.hostnames.joined(separator: ", ")
                )
                WisentField(label: "Admission", value: admission(host))
                WisentField(label: "Running jobs", value: host.runningJobs?.formatted(.number) ?? "Not reported")
                WisentField(label: "CPU", value: cpu(host))
                WisentField(label: "RAM", value: ram(host))
                WisentField(label: "Accelerators available", value: accelerators(host))
                WisentField(label: "VRAM", value: vram(host))
                WisentField(label: "Last capacity report", value: ConsoleFormat.age(host.ageSeconds))
                if host.status == .live {
                    WisentField(label: "Availability", value: host.availabilityReason)
                }
                policySection(for: host)
                WisentActionButton(
                    action: WisentAction(
                        "Reconcile storage roots…",
                        symbol: "externaldrive",
                        kind: .secondary,
                        isEnabled: !reconciliationStore.isRunning && fleetStore.isConfigured
                    ) {
                        let target = host.targetName ?? host.displayName
                        if let address = fleetStore.address {
                            reconciliationTarget = StorageReconciliationTarget(host: target, address: address)
                        }
                    }
                )
            }
        } else {
            WisentInspector(eyebrow: "Selection", title: "No host selected") {
                Text("Select a host to read its capacity report, the reason the fleet gave for its state, why it went quiet the last time it did, and the policy the canonical registry declares for it.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
            }
        }
    }

    /// Policy is shown here and changed in Registry: a compare-and-swap write
    /// belongs beside the generation it is checked against.
    @ViewBuilder
    private func policySection(for host: WorkerNode) -> some View {
        if let target = fleetStore.target(named: host.targetName) {
            WisentField(
                label: "Cleanup mode",
                value: target.cleanup?.mode?.capitalized ?? "Not declared",
                tone: target.cleanup?.mode == FleetCleanupMode.enforce.rawValue ? .warning : .neutral
            )
            WisentField(
                label: "Free space thresholds",
                value: thresholds(target.cleanup)
            )
            WisentField(
                label: "Queue eligibility",
                value: target.pinnedOnly == true
                    ? "Routed jobs only (pinned_only)"
                    : "Any eligible queued job"
            )
            WisentActionButton(
                action: WisentAction("Change policy in Registry", symbol: "book.closed") {
                    route(.registry)
                }
            )
        } else if fleetStore.policy != nil {
            WisentField(
                label: "Canonical registry",
                value: "Not declared",
                tone: .warning
            )
            Text("This host publishes capacity without a matching registry target, so no policy applies to it. Declaring or retiring a host is a registry document write and is not available from this console.")
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.secondary)
        } else {
            WisentField(label: "Canonical registry", value: "Not configured")
        }
    }
}
