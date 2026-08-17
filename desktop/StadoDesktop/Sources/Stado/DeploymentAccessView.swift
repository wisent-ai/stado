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
    @State private var mutation: WisentMutationOutcome = .idle
    @State private var revocation: DeploymentGrant?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider()
            ScrollView {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x6) {
                    WisentMutationBar(outcome: mutation) { mutation = .idle }

                    WisentSectionBox(
                        title: "Grant access",
                        detail: "Grant only the capabilities each person or team needs. Operate covers cleanup passes and policy writes."
                    ) {
                        WisentPanel {
                            VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
                                Picker("Share with", selection: $subject) {
                                    ForEach(AccessSubject.allCases) { value in
                                        Text(value.title).tag(value)
                                    }
                                }
                                .pickerStyle(.segmented)
                                .labelsHidden()

                                subjectFields

                                VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                                    Text("Permissions")
                                        .font(WisentTypeScale.panelTitle())
                                        .foregroundStyle(WisentDesign.ink)
                                    Toggle("View fleet, jobs, and policy", isOn: $canView)
                                    Toggle("Submit jobs", isOn: $canSubmit)
                                    Toggle("Operate cleanup and policy", isOn: $canOperate)
                                    Toggle("Manage deployment and access", isOn: $canAdminister)
                                }
                                .font(WisentTypeScale.body())

                                HStack {
                                    Spacer(minLength: 0)
                                    WisentActionButton(
                                        action: WisentAction(
                                            "Grant access",
                                            symbol: "person.badge.plus",
                                            kind: .primary,
                                            isEnabled: !isSaving && canSubmitGrant
                                        ) {
                                            Task { await grantAccess() }
                                        }
                                    )
                                }
                            }
                        }
                    }

                    WisentSectionBox(
                        title: "Current access",
                        trailing: store.grants[deployment.id].map { "\($0.count.formatted(.number)) grants" }
                    ) {
                        if store.grants[deployment.id] == nil {
                            WisentLoadingPanel(
                                title: "Reading grants",
                                detail: "Who else may read or operate this deployment."
                            )
                        } else if store.grants[deployment.id]?.isEmpty == true {
                            WisentEmptyPanel(
                                title: "Private deployment",
                                detail: "Only the creator has access. No grant exists for this deployment.",
                                symbol: "lock"
                            )
                        } else {
                            WisentTableFrame {
                                VStack(spacing: 0) {
                                    ConsoleTableHead(cells: [
                                        ConsoleHeaderCell("Subject", width: 220),
                                        ConsoleHeaderCell("Permissions"),
                                        ConsoleHeaderCell("Revoke", width: 92, trailing: true),
                                    ])
                                    ForEach(store.grants[deployment.id] ?? []) { grant in
                                        grantRow(grant)
                                    }
                                }
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
        .background(WisentDesign.canvas)
        .task { await store.loadGrants(for: deployment) }
        .sheet(item: $revocation) { grant in
            revocationDialog(grant)
        }
    }

    private var header: some View {
        HStack(alignment: .top, spacing: WisentDesign.Space.x4) {
            VStack(alignment: .leading, spacing: 1) {
                Text("ACCESS")
                    .font(WisentTypeScale.eyebrow())
                    .tracking(0.8)
                    .foregroundStyle(WisentDesign.muted)
                Text(deployment.name)
                    .font(WisentTypography.heading(17))
                    .foregroundStyle(WisentDesign.ink)
                Text(deployment.endpoint ?? "No endpoint published yet")
                    .font(WisentTypeScale.identifierSmall())
                    .foregroundStyle(WisentDesign.secondary)
            }
            Spacer(minLength: 0)
            WisentActionButton(action: WisentAction("Done", kind: .secondary) { dismiss() })
        }
        .padding(WisentDesign.Space.x6)
    }

    @ViewBuilder
    private var subjectFields: some View {
        switch subject {
        case .organization:
            WisentField(
                label: "Organization",
                value: homeOrganization?.name ?? "No organization selected",
                tone: homeOrganization == nil ? .warning : .neutral
            )
        case .role:
            HStack {
                Text("Organization role")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
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
                    .font(WisentTypeScale.identifier())
                Text("The user ID is shown in the teammate's Wisent account profile.")
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.secondary)
            }
        }
    }

    private func grantRow(_ grant: DeploymentGrant) -> some View {
        ConsoleTableRow {
            ConsoleCell(text: subjectLabel(grant), width: 220, strong: true)
            ConsoleCell(text: grant.permissions.map(\.title).joined(separator: " · "))
            HStack {
                Spacer(minLength: 0)
                WisentActionButton(
                    action: WisentAction("Revoke…", kind: .plain, isEnabled: !mutation.isWorking) {
                        revocation = grant
                    }
                )
            }
            .frame(width: 92)
        }
    }

    // MARK: Irreversible decision

    private func revocationDialog(_ grant: DeploymentGrant) -> some View {
        WisentDecisionDialog(
            tone: .danger,
            title: "Revoke \(subjectLabel(grant))'s access to \(deployment.name)?",
            lines: [
                "The subject immediately loses every permission listed below, including any queue submission or cleanup authority it was granted.",
                "Revoking does not stop work this subject already submitted, and the grant cannot be restored from this dialog.",
            ],
            reasonCode: grant.subjectKind,
            listing: [
                "subject: \(grant.subjectID)",
                "permissions: \(grant.permissions.map(\.rawValue).joined(separator: ", "))",
                "granted by: \(grant.createdBy)",
            ],
            footnote: "Granted \(StadoFormat.date(grant.createdAt).map { $0.formatted(date: .abbreviated, time: .shortened) } ?? grant.createdAt).",
            actions: [
                WisentAction("Keep the grant", kind: .primary) { revocation = nil },
                WisentAction("Revoke access", kind: .destructive) {
                    revocation = nil
                    Task { await revoke(grant) }
                },
            ]
        )
    }

    private func revoke(_ grant: DeploymentGrant) async {
        mutation = .working("Revoking access for \(subjectLabel(grant)).")
        do {
            try await store.revoke(grant)
            mutation = .succeeded("Access revoked for \(subjectLabel(grant)).")
        } catch {
            mutation = .failed(Self.describe(error))
        }
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
        mutation = .working("Granting access to \(subject.title.lowercased()).")
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
            mutation = .succeeded("Access granted: \(permissions.map(\.rawValue).joined(separator: ", ")).")
            if subject == .user { userID = "" }
        } catch {
            mutation = .failed(Self.describe(error))
        }
    }

    private func subjectLabel(_ grant: DeploymentGrant) -> String {
        switch grant.subjectKind {
        case "organization": homeOrganization?.name ?? grant.subjectID
        case "organization_role": "\(homeOrganization?.name ?? "Organization") · \((grant.subjectRole ?? "member").capitalized)"
        default: grant.subjectID
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
