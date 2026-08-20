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
    case registry
    case builds
    case releases
    case deployments

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
        case .registry: "Registry"
        case .builds: "Builds"
        case .releases: "Releases"
        case .deployments: "Deployments"
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
        case .registry: "book.closed"
        case .builds: "hammer"
        case .releases: "shippingbox"
        case .deployments: "point.3.connected.trianglepath.dotted"
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
        case .registry: "Canonical fleet policy and the generation it was read at"
        case .builds: "Which repositories the control plane builds on new commits, and what the last build produced"
        case .releases: "What each product should run, what its host runs, and what is holding the rollout"
        case .deployments: "Which Stado backend this console reads, and who else may read it"
        }
    }

    var group: ConsoleGroup {
        switch self {
        case .posture, .queue, .products: .work
        case .hosts, .fleets, .disk, .services: .fleet
        case .registry, .builds, .releases, .deployments: .system
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

// MARK: - Dense table

/// A column head: 9 pt mono, upper case, aligned with the cells below it.
struct ConsoleHeaderCell: Identifiable {
    let id = UUID()
    let title: String
    var width: CGFloat?
    var trailing = false

    init(_ title: String, width: CGFloat? = nil, trailing: Bool = false) {
        self.title = title
        self.width = width
        self.trailing = trailing
    }
}

struct ConsoleTableHead: View {
    let cells: [ConsoleHeaderCell]

    var body: some View {
        HStack(spacing: WisentDesign.Space.x3) {
            ForEach(cells) { cell in
                Text(cell.title.uppercased())
                    .font(WisentTypeScale.columnHead())
                    .tracking(0.7)
                    .foregroundStyle(WisentDesign.muted)
                    .lineLimit(1)
                    .frame(
                        width: cell.width,
                        alignment: cell.trailing ? .trailing : .leading
                    )
                    .frame(maxWidth: cell.width == nil ? .infinity : nil, alignment: cell.trailing ? .trailing : .leading)
            }
        }
        .padding(.horizontal, WisentDesign.Space.x4)
        .frame(height: WisentAppLayout.denseRowHeight)
        .background(WisentDesign.canvasMuted)
        .overlay(alignment: .bottom) {
            Rectangle()
                .fill(WisentDesign.border)
                .frame(height: WisentDesign.hairline)
        }
        .accessibilityHidden(true)
    }
}

/// One 28 pt row. Selection is a background, not a disclosure: the operator
/// keeps every other row in view while reading the inspector.
struct ConsoleTableRow<Content: View>: View {
    private let isSelected: Bool
    private let select: (() -> Void)?
    private let content: () -> Content

    init(
        isSelected: Bool = false,
        select: (() -> Void)? = nil,
        @ViewBuilder content: @escaping () -> Content
    ) {
        self.isSelected = isSelected
        self.select = select
        self.content = content
    }

    var body: some View {
        if let select {
            Button(action: select) { row }
                .buttonStyle(.plain)
                .accessibilityAddTraits(isSelected ? [.isSelected] : [])
        } else {
            row
        }
    }

    private var row: some View {
        HStack(spacing: WisentDesign.Space.x3) {
            content()
        }
        .padding(.horizontal, WisentDesign.Space.x4)
        .frame(height: WisentAppLayout.tableRowHeight)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(isSelected ? WisentDesign.brandSoft : Color.clear)
        .overlay(alignment: .bottom) {
            Rectangle()
                .fill(WisentDesign.border.opacity(0.6))
                .frame(height: WisentDesign.hairline)
        }
        .contentShape(Rectangle())
    }
}

/// A table cell. Identifiers are mono, numbers are monospaced digits, and a
/// tone is only spent on a value that means something is wrong.
struct ConsoleCell: View {
    let text: String
    var width: CGFloat?
    var trailing = false
    var identifier = false
    var digits = false
    var tone: WisentTone = .neutral
    var strong = false

    var body: some View {
        Text(text)
            .font(font)
            .monospacedDigit(digits)
            .foregroundStyle(tone == .neutral ? (strong ? WisentDesign.ink : WisentDesign.secondary) : tone.color)
            .lineLimit(1)
            .truncationMode(.middle)
            .frame(width: width, alignment: trailing ? .trailing : .leading)
            .frame(maxWidth: width == nil ? .infinity : nil, alignment: trailing ? .trailing : .leading)
    }

    private var font: Font {
        if identifier { return WisentTypeScale.identifier() }
        return strong ? WisentTypeScale.bodyStrong() : WisentTypeScale.body()
    }
}

private extension View {
    @ViewBuilder
    func monospacedDigit(_ enabled: Bool) -> some View {
        if enabled { monospacedDigit() } else { self }
    }
}

/// The scrolling body of a data screen: head, rows, nothing else.
struct ConsoleTable<Content: View>: View {
    let head: [ConsoleHeaderCell]
    @ViewBuilder let rows: () -> Content

    var body: some View {
        VStack(spacing: 0) {
            ConsoleTableHead(cells: head)
            ScrollView {
                LazyVStack(spacing: 0) {
                    rows()
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(WisentDesign.surface)
    }
}

// MARK: - Boundary

/// The data boundary, stated once, in the sidebar footer.
///
/// It was repeated on every screen before this; a boundary an operator reads
/// six times is a boundary they stop reading.
struct ConsoleBoundaryFooter: View {
    let sourceName: String
    let sourceDetail: String
    let tone: WisentTone

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            Divider()
            HStack(spacing: WisentDesign.Space.x2) {
                Circle()
                    .fill(tone == .neutral ? WisentDesign.muted : tone.color)
                    .frame(width: 7, height: 7)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 1) {
                    Text(sourceName)
                        .font(WisentTypeScale.bodyStrong())
                        .foregroundStyle(WisentDesign.ink)
                        .lineLimit(1)
                    Text(sourceDetail)
                        .font(WisentTypeScale.identifierSmall())
                        .foregroundStyle(WisentDesign.secondary)
                        .lineLimit(1)
                }
                Spacer(minLength: 0)
            }
            Text("Reads the dashboard state, cleanup, and canonical policy interfaces over HTTP. Writes only a whitelisted policy patch, a cleanup pass, a recorded job rerun, and the allowlisted fleet commands that add a machine. This Mac never opens an SSH session and never reads a private key.")
                .font(WisentTypography.body(10))
                .foregroundStyle(WisentDesign.muted)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(WisentDesign.Space.x3)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(WisentDesign.canvasMuted)
    }
}

// MARK: - Formatting

enum ConsoleFormat {
    /// Ages read as ages; a capacity report from four days ago should not be
    /// spelled "345600 sec".
    static func age(_ seconds: Double?) -> String {
        guard let seconds, seconds.isFinite, seconds >= 0 else { return "Never" }
        return "\(StadoFormat.duration(seconds)) ago"
    }

    static func gigabytes(_ value: Double?) -> String {
        guard let value, value.isFinite else { return "—" }
        return "\(StadoFormat.decimal(value)) GB"
    }

    static func relative(_ date: Date?) -> String {
        guard let date else { return "never" }
        return date.formatted(.relative(presentation: .named))
    }
}
