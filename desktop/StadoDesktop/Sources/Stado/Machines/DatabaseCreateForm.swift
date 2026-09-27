import SwiftUI
import WisentDesignSystem

/// The create form: one `stado database create` invocation. Left empty, the
/// accepted monthly figure makes the CLI refuse with what one more project
/// adds to the bill, which the form shows; the operator then enters that
/// figure to create it. The CLI remains the validator.
struct DatabaseCreateForm: View {
    private enum Layout {
        /// Wide enough for the CLI's cost sentence to wrap in a few lines.
        static let width: CGFloat = 520
    }

    @ObservedObject var store: DatabasesStore
    @Environment(\.dismiss) private var dismiss

    @State private var name = ""
    @State private var consumersText = ""
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
            Text("Creates a Supabase project beside the oko project, writes its <name>-database item and declares it through stado database create. Leave the accepted cost empty to read what it adds to the monthly bill first.")
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
            LabeledContent("Accepted monthly cost (USD)") {
                TextField("empty: report the cost", text: $acceptMonthlyUSD)
                    .textFieldStyle(.roundedBorder)
            }

            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                Button(acceptMonthlyUSD.isEmpty ? "Read cost" : "Create") {
                    isSubmitting = true
                    Task {
                        let consumers = consumersText.split(separator: ",").map(String.init)
                        let created = await store.create(
                            name: name, consumers: consumers, acceptMonthlyUSD: acceptMonthlyUSD
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
