import Foundation

/// The part of adding a machine that is not a form: which way in was chosen,
/// which invitation is outstanding, and which machines have answered it.
///
/// Kept apart from the draft because it has a different lifetime. A draft is
/// one attempt at one machine; this survives an attempt, and an invitation in
/// it can still be answered days after the window that minted it was closed.
struct MachineEnrollmentPlan: Codable, Equatable, Sendable {
    var endpoint = ""
    var flow: MachineEnrollmentFlow = .methods
    /// Which invitation the operator has chosen to mint next. Kept because it
    /// is a decision about the machine in front of them, not a preference: the
    /// window closing between choosing and minting must not silently put them
    /// back on the mode that cannot work for that machine.
    var inviteMode: MachineInviteMode = .online
    var invite: MachineInviteRecord?
    var pending: [FleetPendingRequest] = []
    var pendingReadAt: Date?
    /// The verdict on the last machine approved or rejected here, kept with
    /// its output so the operator can read what approval actually did.
    var decision: MachineEnrollmentCheck?
    /// The machine most recently let into the fleet from this window. It is
    /// what turns "waiting" into "done" on screen: without it, a spent
    /// invitation and an unminted one look identical.
    var approvedName: String?

    var isWaitingForInvite: Bool { invite != nil }

    /// Whether the outstanding invitation is one no machine will ever answer,
    /// which is what decides whether this screen has anything to wait for.
    var isWaitingForOwner: Bool { invite?.isOffline == true }

    /// The request that answered the outstanding invitation, if one has.
    var invitedRequest: FleetPendingRequest? {
        guard let invite, !invite.isOffline else { return nil }
        return pending.first { $0.inviteID == invite.id }
    }

    fileprivate enum CodingKeys: String, CodingKey {
        case endpoint, flow, inviteMode, invite, pending, pendingReadAt, decision, approvedName
    }
}

/// Read leniently for the same reason the invitation record is: this app's own
/// earlier state named no invitation mode, and dropping the whole plan over
/// that would close an open invitation on screen while leaving it open in the
/// store.
extension MachineEnrollmentPlan {
    init(from decoder: Decoder) throws {
        self.init()
        let values = try decoder.container(keyedBy: CodingKeys.self)
        endpoint = try values.decodeIfPresent(String.self, forKey: .endpoint) ?? ""
        flow = try values.decodeIfPresent(MachineEnrollmentFlow.self, forKey: .flow) ?? .methods
        inviteMode = try values.decodeIfPresent(MachineInviteMode.self, forKey: .inviteMode) ?? .online
        invite = try values.decodeIfPresent(MachineInviteRecord.self, forKey: .invite)
        pending = try values.decodeIfPresent([FleetPendingRequest].self, forKey: .pending) ?? []
        pendingReadAt = try values.decodeIfPresent(Date.self, forKey: .pendingReadAt)
        decision = try values.decodeIfPresent(MachineEnrollmentCheck.self, forKey: .decision)
        approvedName = try values.decodeIfPresent(String.self, forKey: .approvedName)
    }
}
