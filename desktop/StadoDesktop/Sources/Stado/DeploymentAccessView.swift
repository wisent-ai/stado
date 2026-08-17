import SwiftUI
import WisentAuth
import WisentDesignSystem

struct DeploymentAccessView: View {
    let deployment: StadoDeployment
    @ObservedObject var store: DeploymentStore
    let homeOrganization: WisentOrganization?

    @Environment(\.dismiss) private var dismiss
    @State private var subject: AccessSubject = .organization
    @State private var userID = ""
    @State private var organizationRole = "member"
    @State private var canView = true
    @State private var canSubmit = true
    @State private var canOperate = false
    @State private var canAdminister = false
    @State private var isSaving = false
    @State private var errorMessage: String?

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                    Text("Access to \(deployment.name)")
                        .font(.title2.weight(.semibold))
                    Text("Grant only the capabilities each person or team needs.")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                Button("Done") { dismiss() }
                    .keyboardShortcut(.defaultAction)
            }
            .padding(WisentDesign.Space.x6)

            Divider()

            ScrollView {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x6) {
                    WisentPanel {
                        VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
                            Text("Share deployment")
                                .font(.headline)
                            Picker("Share with", selection: $subject) {
                                ForEach(AccessSubject.allCases) { value in
                                    Text(value.title).tag(value)
                                }
                            }
                            .pickerStyle(.segmented)

                            subjectFields

                            VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                                Text("Permissions")
                                    .font(.subheadline.weight(.semibold))
                                Toggle("View fleet and jobs", isOn: $canView)
                                Toggle("Submit jobs", isOn: $canSubmit)
                                Toggle("Operate workers and maintenance", isOn: $canOperate)
                                Toggle("Manage deployment and access", isOn: $canAdminister)
                            }

                            if let errorMessage {
                                Label(errorMessage, systemImage: "exclamationmark.triangle.fill")
                                    .font(.caption)
                                    .foregroundStyle(.red)
                            }

                            HStack {
                                Spacer()
                                Button("Grant Access") {
                                    Task { await grantAccess() }
                                }
                                .buttonStyle(.borderedProminent)
                                .disabled(isSaving || !canSubmitGrant)
                            }
                        }
                    }

                    VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                        Text("Current access")
                            .font(.headline)
                        if store.grants[deployment.id] == nil {
                            HStack(spacing: WisentDesign.Space.x3) {
                                ProgressView()
                                Text("Loading grants…")
                                    .foregroundStyle(.secondary)
                            }
                        } else if store.grants[deployment.id]?.isEmpty == true {
                            UnavailableNotice(
                                title: "Private deployment",
                                detail: "Only the creator has access.",
                                symbol: "lock"
                            )
                        } else {
                            ForEach(store.grants[deployment.id] ?? []) { grant in
                                grantRow(grant)
                            }
                        }
                    }
                }
                .padding(WisentDesign.Space.x6)
                .frame(maxWidth: 720)
                .frame(maxWidth: .infinity)
            }
        }
        .frame(minWidth: 700, minHeight: 620)
        .task { await store.loadGrants(for: deployment) }
    }

    @ViewBuilder
    private var subjectFields: some View {
        switch subject {
        case .organization:
            LabeledContent("Organization") {
                Text(homeOrganization?.name ?? "No organization selected")
                    .foregroundStyle(homeOrganization == nil ? .secondary : .primary)
            }
        case .role:
            HStack {
                Text("Organization role")
                Spacer()
                Picker("Organization role", selection: $organizationRole) {
                    Text("Owner").tag("owner")
                    Text("Admin").tag("admin")
                    Text("Member").tag("member")
                }
                .labelsHidden()
                .frame(width: 160)
            }
        case .user:
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                TextField("Wisent user ID", text: $userID)
                    .textFieldStyle(.roundedBorder)
                Text("The user ID is shown in the teammate's Wisent account profile.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }

    private func grantRow(_ grant: DeploymentGrant) -> some View {
        HStack(spacing: WisentDesign.Space.x4) {
            Image(systemName: subjectSymbol(grant.subjectKind))
                .foregroundStyle(.secondary)
                .frame(width: 24)
            VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                Text(subjectLabel(grant))
                    .font(.body.weight(.medium))
                Text(grant.permissions.map(\.title).joined(separator: " · "))
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            Button(role: .destructive) {
                Task {
                    do {
                        try await store.revoke(grant)
                    } catch {
                        errorMessage = Self.describe(error)
                    }
                }
            } label: {
                Image(systemName: "trash")
            }
            .buttonStyle(.borderless)
            .help("Revoke access")
        }
        .padding(WisentDesign.Space.x3)
        .background(WisentDesign.surface, in: RoundedRectangle(cornerRadius: WisentDesign.Radius.medium))
    }

    private var canSubmitGrant: Bool {
        !permissions.isEmpty
            && (subject != .organization || homeOrganization != nil)
            && (subject != .user || UUID(uuidString: userID.trimmingCharacters(in: .whitespacesAndNewlines)) != nil)
    }

    private var permissions: [DeploymentPermission] {
        var values: [DeploymentPermission] = []
        if canView { values.append(.view) }
        if canSubmit { values.append(.submit) }
        if canOperate { values.append(.operate) }
        if canAdminister { values.append(.admin) }
        return values
    }

    private func grantAccess() async {
        guard canSubmitGrant else { return }
        isSaving = true
        errorMessage = nil
        defer { isSaving = false }
        do {
            let organizationID = homeOrganization?.id ?? ""
            try await store.share(
                deployment: deployment,
                subjectKind: subject.apiKind,
                subjectID: subject == .user ? userID.trimmingCharacters(in: .whitespacesAndNewlines) : organizationID,
                subjectRole: subject == .role ? organizationRole : nil,
                permissions: permissions
            )
            if subject == .user { userID = "" }
        } catch {
            errorMessage = Self.describe(error)
        }
    }

    private func subjectLabel(_ grant: DeploymentGrant) -> String {
        switch grant.subjectKind {
        case "organization": homeOrganization?.name ?? grant.subjectID
        case "organization_role": "\(homeOrganization?.name ?? "Organization") · \((grant.subjectRole ?? "member").capitalized)"
        default: grant.subjectID
        }
    }

    private func subjectSymbol(_ kind: String) -> String {
        switch kind {
        case "organization": "building.2"
        case "organization_role": "person.3"
        default: "person"
        }
    }

    private static func describe(_ error: Error) -> String {
        (error as? LocalizedError)?.errorDescription ?? "Access could not be updated."
    }
}

private enum AccessSubject: String, CaseIterable, Identifiable {
    case organization
    case role
    case user

    var id: Self { self }

    var title: String {
        switch self {
        case .organization: "Organization"
        case .role: "Role"
        case .user: "User"
        }
    }

    var apiKind: String {
        switch self {
        case .organization: "organization"
        case .role: "organization_role"
        case .user: "user"
        }
    }
}

private extension DeploymentPermission {
    var title: String {
        switch self {
        case .view: "View"
        case .submit: "Submit"
        case .operate: "Operate"
        case .admin: "Admin"
        }
    }
}
