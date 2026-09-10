import SwiftUI
import WisentDesignSystem

/// Sidebar groups. Three of them, each answering a different question the
/// operator arrives with: what needs me now, what is the fleet made of, and
/// what is this console reading.
enum ConsoleGroup: String, CaseIterable, Identifiable {
    case work = "Work"
    case fleet = "Fleet"
    case system = "System"

    var id: String { rawValue }
}

/// A destination exists because something is decided or verified there, never
/// because the backend happens to publish a noun.
enum ConsoleDestination: String, CaseIterable, Identifiable {
    case posture
    case queue
    case products
    case hosts
    case fleets
    case services
    case disk
    case memory
    case databases
    case registry
    case builds
    case releases
    case deployments
    case inference
    case cloudflare

    var id: String { rawValue }

    var title: String {
        switch self {
        case .posture: "Posture"
        case .queue: "Queue"
        case .products: "Products"
        case .hosts: "Hosts"
        case .fleets: "Fleets"
        case .services: "Services"
        case .disk: "Disk"
        case .memory: "Memory"
        case .databases: "Databases"
        case .registry: "Registry"
        case .builds: "Builds"
        case .releases: "Releases"
        case .deployments: "Deployments"
        case .cloudflare: "Cloudflare routes"
        case .inference: "Inference"
        }
    }

    var symbol: String {
        switch self {
        case .posture: "bell.badge"
        case .queue: "list.bullet.rectangle"
        case .products: "shippingbox"
        case .hosts: "server.rack"
        case .fleets: "rectangle.3.group"
        case .services: "gearshape.2"
        case .disk: "externaldrive"
        case .memory: "memorychip"
        case .databases: "cylinder"
        case .registry: "book.closed"
        case .builds: "hammer"
        case .releases: "shippingbox"
        case .deployments: "point.3.connected.trianglepath.dotted"
        case .cloudflare: "network"
        case .inference: "cpu"
        }
    }

    var purpose: String {
        switch self {
        case .posture: "What in the fleet needs a human right now"
        case .queue: "Queued work by model and the outcome of every recent job"
        case .products: "Install, update, roll back and remove canonical Wisent products"
        case .hosts: "Which hosts can take work, and why the others cannot"
        case .fleets: "Named groups of machines: declare one, assign machines, retire one"
        case .services: "What each declared unit runs, and which processes nothing owns"
        case .disk: "Disk pressure, what the last pass reclaimed, and the next pass"
        case .memory: "Host memory, swap, the declared reclaim policy, and whether this host still accepts jobs"
        case .databases: "Declared fleet databases, their placement and who may resolve them"
        case .registry: "Canonical fleet policy and the generation it was read at"
        case .builds: "Which repositories the control plane builds on new commits, and what the last build produced"
        case .releases: "What each product should run, what its host runs, and what is holding the rollout"
        case .deployments: "Which Stado backend this console reads, and who else may read it"
        case .cloudflare: "Publish a hostname through a declared Cloudflare Tunnel connector"
        case .inference: "Which model each router alias reaches, and the declared deployments behind it"
        }
    }

    var group: ConsoleGroup {
        switch self {
        case .posture, .queue, .products: .work
        case .hosts, .fleets, .disk, .memory, .services, .inference: .fleet
        case .databases, .registry, .builds, .releases, .cloudflare, .deployments: .system
        }
    }

    static func members(of group: ConsoleGroup) -> [ConsoleDestination] {
        allCases.filter { $0.group == group }
    }
}

/// Which destination the window is showing. Shared so the menu bar can send an
/// operator straight to the screen that owns a decision instead of duplicating
/// the decision in a popover.
@MainActor
final class ConsoleRouter: ObservableObject {
    @Published var destination: ConsoleDestination = .posture
    /// The host a screen named while sending the operator somewhere else, so a
    /// decision row about one machine lands on that machine's row rather than
    /// on a table of twelve. Cleared by the destination once it has selected
    /// it: after the jump the selection is the operator's.
    @Published var focusedHost: String?

    func show(_ destination: ConsoleDestination, host: String? = nil) {
        focusedHost = host
        self.destination = destination
    }
}

