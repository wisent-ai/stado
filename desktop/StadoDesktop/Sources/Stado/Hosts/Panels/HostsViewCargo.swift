import SwiftUI
import WisentDesignSystem

extension HostsView {
    @ViewBuilder
    func cargoInventorySection(for host: WorkerNode) -> some View {
        let target = host.targetName ?? host.displayName
        let inventory = inventoryStore.cargo(for: target)
        let reading = inventoryStore.isReading(target)
        WisentSectionBox(
            title: "Cargo home",
            detail: "Live metadata from the fixed $HOME/.cargo and $HOME/.cargo/bin paths. Symlink targets and direct bin children are metadata only; no file contents are read.",
            trailing: reading ? "Reading…" : (inventory.map { $0.complete ? "Complete" : "Incomplete" } ?? "Not read")
        ) {
            if let failure = inventoryStore.failure(for: target) {
                WisentAlertPanel(
                    tone: .warning,
                    title: "Cargo inventory could not be read",
                    detail: failure
                )
            }
            if let inventory {
                WisentField(
                    label: "$HOME/.cargo",
                    value: cargoMetadataDescription(inventory.home),
                    tone: inventory.home.metadataState == "read" || inventory.home.metadataState == "missing"
                        ? .neutral
                        : .warning
                )
                WisentField(
                    label: "$HOME/.cargo/bin",
                    value: cargoMetadataDescription(inventory.bin),
                    tone: inventory.bin.metadataState == "read" || inventory.bin.metadataState == "missing"
                        ? .neutral
                        : .warning
                )
                WisentField(
                    label: "Bin membership",
                    value: "\(inventory.entries.count.formatted(.number)) listed of \(inventory.entriesSeen.formatted(.number)) seen · \(inventory.entriesState)",
                    tone: inventory.entriesComplete ? .neutral : .warning
                )
                if inventory.entries.isEmpty {
                    WisentField(label: "Bin entries", value: "None reported")
                } else {
                    ForEach(inventory.entries.indices, id: \.self) { index in
                        let entry = inventory.entries[index]
                        WisentField(
                            label: entry.name,
                            value: cargoMetadataDescription(entry),
                            tone: entry.metadataState == "read"
                                && entry.nameState == "read"
                                && (entry.symlinkTargetState == "read"
                                    || entry.symlinkTargetState == "not_symlink")
                                ? .neutral
                                : .warning
                        )
                    }
                }
                if !inventory.complete {
                    WisentAlertPanel(
                        tone: .warning,
                        title: "Cargo inventory is incomplete",
                        detail: "Stado did not claim that the rows above are the whole fixed Cargo inventory. The membership and per-entry states name the refused, partial, malformed, or truncated read."
                    )
                }
            } else if !host.declared && !reading {
                WisentField(
                    label: "Cargo inventory",
                    value: "Not asked: this host is not a declared registry target",
                    tone: .warning
                )
            }
            WisentActionButton(
                action: WisentAction(
                    inventory == nil ? "Read Cargo inventory" : "Refresh Cargo inventory",
                    symbol: "shippingbox",
                    kind: .secondary,
                    isEnabled: host.declared && !reading
                ) {
                    Task { await inventoryStore.refresh(host: target) }
                }
            )
        }
    }

    private func cargoMetadataDescription(_ metadata: HostFilesystemMetadata) -> String {
        var values = [
            "type \(metadata.kind)",
            "metadata \(metadata.metadataState)",
            "mode \(metadata.mode)",
            "uid \(metadata.uid.map { String($0) } ?? "unavailable")",
            "gid \(metadata.gid.map { String($0) } ?? "unavailable")",
            "size \(metadata.bytes.map { "\($0) bytes" } ?? "unavailable")",
            "mtime \(metadata.modifiedEpoch.map { String($0) } ?? "unavailable")",
        ]
        if metadata.nameState != "read" {
            values.append("name \(metadata.nameState)")
        }
        if metadata.kind == "symlink" || metadata.symlinkTargetState != "not_symlink" {
            let target = metadata.symlinkTarget.isEmpty ? "unavailable" : metadata.symlinkTarget
            values.append("link \(target) (\(metadata.symlinkTargetState))")
        }
        return values.joined(separator: " · ")
    }
}
