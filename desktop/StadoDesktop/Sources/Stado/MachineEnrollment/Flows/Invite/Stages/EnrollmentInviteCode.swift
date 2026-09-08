import SwiftUI
import WisentDesignSystem

/// The half of the invite screen that exists for one minute: the code, shown
/// once, and where the line it stands in came from.
///
/// `code` is internal rather than private because `body` is in the file above
/// this folder; `probedCaption` stays private, its only caller being here.
extension EnrollmentInviteView {
    @ViewBuilder
    func code(_ invite: MachineInvite) -> some View {
        WisentAlertPanel(
            tone: .warning,
            title: "This code is shown once",
            detail: "It is not written to disk, not kept in this window's saved state, and cannot be printed again by any command. Send it now. If it is lost, revoke this invitation and mint another — that costs one message, and reading a secret back from a file would cost the fleet."
        )

        WisentSectionBox(
            title: "One line to send to whoever has the machine",
            detail: "They run it on the machine being added. It fetches the join script from this control plane, installs the fleet's public key into their authorized_keys, and reports the machine back here.",
            trailing: "uses \(invite.usesAllowed)"
        ) {
            EnrollmentCopyBlock(
                text: invite.joinCommand,
                caption: probedCaption(invite),
                isSecret: true
            )
        }

        if invite.baseIsTemporary, !invite.baseWarning.isEmpty {
            // The control plane's own sentence, verbatim: it names the exact
            // address and the exact way the line dies. A paraphrase would
            // drift the moment the CLI wording changes.
            WisentAlertPanel(
                tone: .warning,
                title: "This line stands on a temporary entrance (address source: \(invite.baseSource))",
                detail: invite.baseWarning
            )
        }

        WisentSectionBox(
            title: "The code on its own",
            detail: "For a person who would rather paste the code into a prompt than run a line they did not write."
        ) {
            EnrollmentCopyBlock(text: invite.token, isSecret: true)
        }

        if !invite.authorizedKeysLine.isEmpty {
            WisentSectionBox(
                title: "Or the key, by hand",
                detail: "The same public key the script installs. Somebody who will not run a script can append this line to ~/.ssh/authorized_keys on the machine instead, and you can then adopt it by address."
            ) {
                EnrollmentCopyBlock(text: invite.authorizedKeysLine)
            }
        }
    }

    /// Where the line came from, naming the address it was built against when
    /// the control plane reported one. The fact belongs beside the line rather
    /// than in a panel of its own: a green alert box with a warning triangle
    /// over good news is how operators learn to stop reading alert boxes.
    private func probedCaption(_ invite: MachineInvite) -> String {
        guard let checkpoint = invite.checkpoint, !checkpoint.url.isEmpty else {
            return "Assembled by the control plane against its own configured address, not by this app, and only after that address served the join script."
        }
        return "Assembled by the control plane against \(checkpoint.url), which served /join.sh when this was minted. This app did not build the line and does not know that address."
    }
}
