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
            trailing: cargoTrailing(reading: reading, inventory: inventory)
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
                    tone: inventory.home.metadataAnswered
                        ? .neutral
                        : .warning
                )
                WisentField(
                    label: "$HOME/.cargo/bin",
                    value: cargoMetadataDescription(inventory.bin),
                    tone: inventory.bin.metadataAnswered
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
                            tone: entry.readExactly
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

    /// The trailing word of the cargo section: reading, the inventory's own
    /// completeness once read, or that no inventory has been read.
    private func cargoTrailing(reading: Bool, inventory: HostCargoInventory?) -> String {
        if reading { return "Reading…" }
        switch inventory {
        case .some(let inventory): return inventory.complete ? "Complete" : "Incomplete"
        case .none: return "Not read"
        }
    }

    private func cargoMetadataDescription(_ metadata: HostFilesystemMetadata) -> String {
        var values = [
            "type \(metadata.kind)",
            "metadata \(metadata.metadataState)",
            "mode \(metadata.mode)",
        ]
        // The stat facts are written only when the metadata state is `read`;
        // in every other state the inventory writes them as null, and the
        // state word above says why.
        if let uid = metadata.uid, let gid = metadata.gid, let bytes = metadata.bytes,
            let modified = metadata.modifiedEpoch
        {
            values += ["uid \(uid)", "gid \(gid)", "size \(bytes) bytes", "mtime \(modified)"]
        }
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
