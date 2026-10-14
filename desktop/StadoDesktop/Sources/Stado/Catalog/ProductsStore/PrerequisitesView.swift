import SwiftUI
import WisentDesignSystem

struct ProductPrerequisitesView: View {
    @ObservedObject var store: ProductsStore
    let product: String
    let surface: String
    let host: String?
    let releaseVersion: String?
    let sourceCommit: String?
    let onInstall: (String?, String?) -> Void

    private func selected(_ value: String?) -> String? {
        let trimmed = value?.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed?.isEmpty == false ? trimmed : nil
    }

    private var version: String? { selected(releaseVersion) }
    private var commit: String? { selected(sourceCommit) }
    private var key: ProductPlanKey {
        ProductPlanKey(product: product, surface: surface, host: host,
                       releaseVersion: version, sourceCommit: commit)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            WisentActionButton(action: WisentAction("Plan prerequisites", symbol: "list.bullet",
                                                   kind: .secondary, isEnabled: surface != "service" || host?.isEmpty == false) {
                Task { await store.plan(product: product, surface: surface, host: host,
                                        releaseVersion: version, sourceCommit: commit) }
            })
            if let failure = store.planFailures[key] {
                WisentErrorBanner(title: "Prerequisite plan refused", detail: failure)
            }
            if let plan = store.plans[key] {
                ForEach(plan.chain.indices, id: \.self) { index in
                    let step = plan.chain[index]
                    VStack(alignment: .leading) {
                        Text("\(step.product)/\(step.surface): \(step.action) \(step.revision)")
                        if let verb = step.verb { Text("Verb \(verb)") }
                        if let minimum = step.minimumRevision { Text("Minimum \(minimum)") }
                        if let installed = step.installedRevision { Text("Installed \(installed)") }
                        if let reason = step.reason { Text(reason) }
                    }.font(WisentTypeScale.identifierSmall()).textSelection(.enabled)
                }
                if plan.chain.contains(where: { $0.verb != nil && $0.action == "install" }) {
                    WisentActionButton(action: WisentAction("Install with prerequisites",
                                                           symbol: "arrow.down.circle", kind: .secondary,
                                                           isEnabled: !store.mutation.isWorking) {
                        onInstall(version, commit)
                    })
                }
            }
        }
    }
}
