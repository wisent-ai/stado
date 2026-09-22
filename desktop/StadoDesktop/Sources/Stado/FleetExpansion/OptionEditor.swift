import SwiftUI

struct FleetExpansionOptionEditor: View {
    let existing: FleetExpansionOption?
    let needs: [FleetNeed]
    let save: (FleetExpansionOption) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var identifier = ""
    @State private var label = ""
    @State private var kind = "buy"
    @State private var keys = ""
    @State private var benefitGroup = ""
    @State private var upfront = ""
    @State private var monthlyCost = ""
    @State private var savings = ""
    @State private var margin = ""
    @State private var lead = ""
    @State private var evidence = ""
    @State private var observed = ""
    @State private var expires = ""
    @State private var failure: String?

    var body: some View {
        VStack(alignment: .leading) {
            Text(existing == nil ? "Add expansion option" : "Edit expansion option").font(.title2)
            Text("One option is a complete bundle. Empty money fields remain unknown. This form records assumptions, not an order.")
            Form {
                TextField("Identifier", text: $identifier).disabled(existing != nil)
                TextField("Description", text: $label)
                Picker("Method", selection: $kind) {
                    ForEach(["buy", "upgrade", "rent", "reclaim", "relocate"], id: \.self) { Text($0).tag($0) }
                }
                TextField("Need keys, comma separated", text: $keys)
                ForEach(needs) { need in
                    Button("Add \(need.expansionKey)") {
                        let present = keys.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }
                        if !present.contains(need.expansionKey) { keys = (present + [need.expansionKey]).joined(separator: ", ") }
                    }
                }
                TextField("Benefit group — shared savings use the same group", text: $benefitGroup)
                TextField("Upfront cost (USD; blank = unknown)", text: $upfront)
                TextField("Monthly operating cost (USD)", text: $monthlyCost)
                TextField("Monthly avoided spending (USD)", text: $savings)
                TextField("Monthly additional margin (USD)", text: $margin)
                TextField("Delivery delay (days)", text: $lead)
                TextField("Source and basis for costs, benefits and expected capacity gain", text: $evidence, axis: .vertical)
                TextField("Observed at (RFC3339)", text: $observed)
                TextField("Valid until (RFC3339)", text: $expires)
            }
            if let failure { Text(failure).foregroundStyle(.red).textSelection(.enabled) }
            HStack {
                Button("Cancel") { dismiss() }
                Spacer()
                Button("Keep in draft") { keep() }.keyboardShortcut(.defaultAction)
            }
        }
        .padding()
        .frame(minWidth: FleetExpansionDefaults.width / 2)
        .onAppear { populate() }
    }

    private func populate() {
        guard let option = existing else {
            observed = ISO8601DateFormatter().string(from: Date())
            return
        }
        identifier = option.id; label = option.label; kind = option.kind
        keys = option.needKeys.joined(separator: ", "); benefitGroup = option.benefitGroup
        upfront = option.upfrontUsd.map(String.init(describing:)) ?? ""
        monthlyCost = option.monthlyCostUsd.map(String.init(describing:)) ?? ""
        savings = option.monthlySavingsUsd.map(String.init(describing:)) ?? ""
        margin = option.monthlyMarginUsd.map(String.init(describing:)) ?? ""
        lead = String(option.leadTimeDays); evidence = option.evidence
        observed = option.observedAt; expires = option.validUntil
    }

    private func amount(_ raw: String) throws -> Double? {
        let text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if text.isEmpty { return nil }
        guard let value = Double(text), value.isFinite, value >= 0 else {
            throw FleetExpansionFailure("Money must be a nonnegative number or blank for unknown.")
        }
        return value
    }

    private func keep() {
        do {
            guard let delay = Int(lead), delay >= 0 else { throw FleetExpansionFailure("Delivery delay must be a nonnegative whole number of days.") }
            let option = try FleetExpansionOption(id: identifier, label: label, kind: kind,
                needKeys: keys.split(separator: ",").map { $0.trimmingCharacters(in: .whitespacesAndNewlines) },
                benefitGroup: benefitGroup, upfrontUsd: amount(upfront), monthlyCostUsd: amount(monthlyCost),
                monthlySavingsUsd: amount(savings), monthlyMarginUsd: amount(margin), leadTimeDays: delay,
                evidence: evidence, observedAt: observed, validUntil: expires)
            save(option)
            dismiss()
        } catch { failure = error.localizedDescription }
    }
}
