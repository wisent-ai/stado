import SwiftUI
import WisentDesignSystem

/// The declared memory policies written for this host, and the backend's
/// verdict on the declaration it currently carries.
///
/// It exists because a readable declaration is not a managed host. On
/// 2026-09-06 charless-mac-mini carried a `report`-mode policy whose one
/// repair could never fire, and every surface showed its fields without
/// saying that nothing would ever be repaired. The verdict here is the
/// backend's own sentence, and the policies offered are the fleet's declared
/// ones, so arming a host from this window writes the document `stado space
/// watermark --policy` writes.
struct MemoryDeclaredPoliciesSection: View {
    let state: MemoryPolicyState
    /// The whole catalog; the fit for this host is the backend's list.
    let declared: [DeclaredMemoryPolicy]
    let fit: FleetMemoryPolicyFit?
    let isWriting: Bool
    /// Fill the editor with a declared policy, which is where the operator
    /// reviews and posts it.
    let fill: (DeclaredMemoryPolicy) -> Void

    /// The processes an operator has authorized ending, by policy name. The
    /// same two-declaration rule the CLI enforces: the catalog names them,
    /// and the operator authorizes them here.
    @State private var authorized: Set<String> = []

    var body: some View {
        WisentSectionBox(
            title: "Declared policies",
            detail: fit?.automatic.detail ?? "The registry projection carried no verdict for this host.",
            trailing: trailing
        ) {
            if fitting.isEmpty {
                WisentField(
                    label: "Nothing fits this host",
                    value: "No declared policy names this host's platform and role, so it can only be edited field by field below."
                )
            }
            ForEach(fitting) { policy in
                row(policy)
            }
        }
    }

    /// Which declared policies this host may be armed with, in catalog order.
    private var fitting: [DeclaredMemoryPolicy] {
        let names = Set(fit?.fitting ?? [])
        return declared.filter { names.contains($0.name) }
    }

    private var trailing: String {
        guard let automatic = fit?.automatic else { return "no verdict" }
        if let reviewed = automatic.reviewedPolicy {
            return automatic.armed ? "armed · \(reviewed)" : "declared · \(reviewed)"
        }
        return state.declared == nil ? "undeclared" : "written by hand"
    }

    @ViewBuilder
    private func row(_ policy: DeclaredMemoryPolicy) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
            HStack(alignment: .firstTextBaseline, spacing: WisentDesign.Space.x2) {
                Text(policy.name)
                    .font(WisentTypeScale.bodyStrong())
                Text(policy.headline)
                    .font(WisentTypeScale.eyebrow())
                    .foregroundStyle(WisentDesign.muted)
                Spacer(minLength: WisentDesign.Space.x2)
                if fit?.automatic.reviewedPolicy == policy.name {
                    Text("in force")
                        .font(WisentTypeScale.eyebrow())
                        .foregroundStyle(WisentDesign.muted)
                } else {
                    Button("Fill the editor") { fill(policy) }
                        .buttonStyle(WisentSecondaryButtonStyle())
                        .disabled(isWriting || !isAuthorized(policy))
                }
            }
            Text(policy.summary)
                .font(WisentTypeScale.body())
                .foregroundStyle(WisentDesign.secondary)
                .fixedSize(horizontal: false, vertical: true)
            if policy.endsGraphicalSession {
                Toggle(isOn: authorization(for: policy)) {
                    Text(
                        "Authorize ending \(policy.sessionProcesses.joined(separator: ", ")) on \(state.target)"
                    )
                    .font(WisentTypeScale.eyebrow())
                }
                .toggleStyle(.checkbox)
            }
        }
        .padding(.vertical, WisentDesign.Space.x1)
    }

    private func isAuthorized(_ policy: DeclaredMemoryPolicy) -> Bool {
        !policy.endsGraphicalSession || authorized.contains(policy.name)
    }

    private func authorization(for policy: DeclaredMemoryPolicy) -> Binding<Bool> {
        Binding(
            get: { authorized.contains(policy.name) },
            set: { permitted in
                if permitted {
                    authorized.insert(policy.name)
                } else {
                    authorized.remove(policy.name)
                }
            }
        )
    }
}
