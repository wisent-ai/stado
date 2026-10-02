import SwiftUI
import WisentDesignSystem

/// The create form: one `stado database create` invocation on any provider.
/// Fleet runs postgres or sqlite on a fleet host; supabase creates a hosted
/// project and, with the accepted monthly figure left empty, shows the CLI's
/// sentence naming what it adds to the bill; external brings a server of any
/// engine, whose connection URL goes to the CLI's standard input. The CLI
/// remains the validator.
struct DatabaseCreateForm: View {
    private enum Layout {
        /// Wide enough for the CLI's cost sentence to wrap in a few lines.
        static let width: CGFloat = 520
    }

    @ObservedObject var store: DatabasesStore
    @Environment(\.dismiss) private var dismiss

    @State private var name = ""
    @State private var consumersText = ""
    @State private var provider = "fleet"
    @State private var engine = ""
    @State private var host = ""
    @State private var caCertificatePath = ""
    @State private var connectionURL = ""
    @State private var acceptMonthlyUSD = ""
    @State private var isSubmitting = false

    private var nameIsValid: Bool {
        !name.isEmpty && name.allSatisfy { $0.isLowercase || $0.isNumber || $0 == "-" }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
            Text("Create a database")
                .font(WisentTypeScale.section())
                .foregroundStyle(WisentDesign.ink)
            Text("Creates the database through stado database create, writes its <name>-database item and declares it. Leave the engine empty for postgres on fleet and supabase, or for the engine the connection URL names on external.")
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.muted)
            if let problem = store.problem {
                Text(problem)
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            LabeledContent("Name (lowercase, digits, dashes)") {
                TextField("skryba", text: $name)
                    .textFieldStyle(.roundedBorder)
            }
            LabeledContent("Consumers (comma-separated)") {
                TextField("skryba", text: $consumersText)
                    .textFieldStyle(.roundedBorder)
            }
            Picker("Provider", selection: $provider) {
                Text("fleet").tag("fleet")
                Text("supabase").tag("supabase")
                Text("external").tag("external")
            }
            .pickerStyle(.segmented)
            LabeledContent("Engine (optional)") {
                TextField("postgres, sqlite, mysql, mongodb, redis…", text: $engine)
                    .textFieldStyle(.roundedBorder)
            }
            if provider == "fleet" {
                LabeledContent("Fleet host (optional)") {
                    TextField("empty: the vault owner", text: $host)
                        .textFieldStyle(.roundedBorder)
                }
            }
            if provider == "supabase" {
                LabeledContent("Accepted monthly cost (USD)") {
                    TextField("empty: report the cost", text: $acceptMonthlyUSD)
                        .textFieldStyle(.roundedBorder)
                }
            }
            if provider == "external" {
                LabeledContent("Connection URL") {
                    SecureField("engine://user:password@host:port/database", text: $connectionURL)
                        .textFieldStyle(.roundedBorder)
                }
                LabeledContent("Server CA certificate (PEM path)") {
                    TextField("/path/to/server-ca.pem", text: $caCertificatePath)
                        .textFieldStyle(.roundedBorder)
                }
            }

            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                Button(provider == "supabase" && acceptMonthlyUSD.isEmpty ? "Read cost" : "Create") {
                    isSubmitting = true
                    Task {
                        let consumers = consumersText.split(separator: ",").map(String.init)
                        let created = await store.create(
                            name: name,
                            consumers: consumers,
                            provider: provider,
                            engine: engine,
                            host: provider == "fleet" ? host : "",
                            caCertificatePath: provider == "external" ? caCertificatePath : "",
                            acceptMonthlyUSD: provider == "supabase" ? acceptMonthlyUSD : "",
                            connectionURL: provider == "external" ? connectionURL : ""
                        )
                        if created { dismiss() }
                        isSubmitting = false
                    }
                }
                .disabled(!nameIsValid || consumersText.isEmpty || isSubmitting)
            }
        }
        .padding(WisentDesign.Space.x6)
        .frame(width: Layout.width)
    }
}

/// The declare form: name, engine, scopes and initial consumers. Every field
/// maps onto one `stado database declare` invocation; the CLI remains the
/// validator, this form only assembles its arguments.
struct DatabaseDeclareForm: View {
    @ObservedObject var store: DatabasesStore
    @Environment(\.dismiss) private var dismiss

    @State private var name = ""
    @State private var engine = "postgres"
    @State private var readScope = true
    @State private var writeScope = false
    @State private var consumersText = ""
    @State private var isSubmitting = false

    private var nameIsValid: Bool {
        !name.isEmpty
            && name == name.trimmingCharacters(in: .whitespaces)
            && name.allSatisfy { $0.isLowercase || $0.isNumber || $0 == "-" }
    }

    private var scopes: [String] {
        var values: [String] = []
        if readScope { values.append("read") }
        if writeScope { values.append("write") }
        return values
    }

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
            Text("Declare a database")
                .font(WisentTypeScale.section())
                .foregroundStyle(WisentDesign.ink)
            Text("Writes database_api.databases into the Stado configuration through stado database declare. Provision the credential item <name>-database with stado credentials put.")
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.muted)

            LabeledContent("Name (lowercase, digits, dashes)") {
                TextField("echo", text: $name)
                    .textFieldStyle(.roundedBorder)
            }
            LabeledContent("Engine") {
                TextField("postgres, sqlite, mysql, mongodb, redis…", text: $engine)
                    .textFieldStyle(.roundedBorder)
            }
            HStack(spacing: WisentDesign.Space.x5) {
                Toggle("read", isOn: $readScope)
                Toggle("write", isOn: $writeScope)
            }
            LabeledContent("Consumers (comma-separated)") {
                TextField("echo-desktop", text: $consumersText)
                    .textFieldStyle(.roundedBorder)
            }

            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                Button("Declare") {
                    isSubmitting = true
                    Task {
                        let consumers = consumersText.split(separator: ",").map(String.init)
                        let declared = await store.declare(
                            name: name,
                            engine: engine,
                            scopes: scopes,
                            consumers: consumers
                        )
                        if declared { dismiss() }
                        isSubmitting = false
                    }
                }
                .disabled(!nameIsValid || scopes.isEmpty || isSubmitting)
            }
        }
        .padding(WisentDesign.Space.x6)
        .frame(width: 480)
    }
}

/// The push form: one `stado database push HOST --service UNIT`. The host's
/// config file is read whole and its database declarations are made equal
/// to this machine's; the named unit is reconciled so it reads them.
struct DatabasePushForm: View {
    @ObservedObject var store: DatabasesStore
    @Environment(\.dismiss) private var dismiss
    @State private var host = ""
    @State private var service = ""
    @State private var isSubmitting = false

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
            Text("Push declarations to a host")
                .font(WisentTypeScale.section())
                .foregroundStyle(WisentDesign.ink)
            Text("Runs stado database push: the host's database_api becomes this machine's, then the named unit is reconciled so its running process reads it.")
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.muted)
            LabeledContent("Host") {
                TextField("registry host", text: $host)
                    .textFieldStyle(.roundedBorder)
            }
            LabeledContent("Unit serving the database plane") {
                TextField("com.wisent.stado", text: $service)
                    .textFieldStyle(.roundedBorder)
            }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                Button("Push") {
                    isSubmitting = true
                    Task {
                        if await store.push(host: host, service: service) { dismiss() }
                        isSubmitting = false
                    }
                }
                .disabled(host.isEmpty || service.isEmpty || isSubmitting)
            }
        }
        .padding(WisentDesign.Space.x6)
        .frame(width: 480)
    }
}
