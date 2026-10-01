import Foundation

/// A failure told as what it is.
///
/// `enroll` probes the machine before it writes anything and rolls its own
/// entry back when the agent install fails, so its two failure modes mean
/// opposite things to the operator: one leaves the registry untouched, the
/// other leaves it untouched after having briefly touched it. Neither means
/// "go looking for a half-added machine", and a bare backend sentence does not
/// say so on its own.
struct MachineEnrollmentFailure: Equatable, Sendable {
    let title: String
    let detail: String
    let backendMessage: String

    static func keyGeneration(_ message: String, machine: String) -> Self {
        Self(
            title: "The key pair for \(machine) was not stored",
            detail: "Nothing was written to the registry and nothing was changed on any machine. The key lives in the credential store, so this failure is about that store, not about the fleet.",
            backendMessage: message
        )
    }

    static func missingPublicKey(machine: String) -> Self {
        Self(
            title: "The key was minted but its public half was not printed",
            detail: "Stado reported success without a public key line, so there is nothing to put on \(machine). Run the step again; if it repeats, read the credential item directly with stado fleet key ls.",
            backendMessage: ""
        )
    }

    /// Enrollment probes the machine before it writes anything and rolls its
    /// own entry back when the agent install fails, so whatever the command
    /// refused, no half-added machine is left behind. Which refusal it was is
    /// the command's own sentence, shown verbatim; it is not guessed here from
    /// its words.
    static func enrollment(_ message: String, machine: String, sshTarget: String) -> Self {
        Self(
            title: "\(machine) was not enrolled",
            detail: "Enrollment asks \(sshTarget) for its hostname, uname -s and uname -m before it writes anything, and rolls its own registry entry back if the agent install fails, so the registry is exactly as it was before this attempt. Stado's own sentence below says what refused.",
            backendMessage: message
        )
    }

    static func transport(_ message: String) -> Self {
        Self(
            title: "The Stado dashboard did not run the command",
            detail: "The command bridge answered with a refusal or could not be reached, so nothing was attempted on the fleet.",
            backendMessage: message
        )
    }

    /// The list of ways in could not be read. Without it there is no screen,
    /// so this failure has to name the one thing that would explain it: an
    /// older control plane than this app.
    static func methods(_ message: String) -> Self {
        Self(
            title: "This Stado did not report its enrollment methods",
            detail: "The app asks the control plane which ways into the fleet exist rather than carrying its own list, and this control plane did not answer with one. A release older than stado fleet methods answers exactly like this. Nothing was attempted on the fleet.",
            backendMessage: message
        )
    }

    static func invite(_ message: String, machine: String) -> Self {
        Self(
            title: "No invitation was minted for \(machine)",
            detail: "Minting writes one object to the store and one key pair to the credential store, in that order, and neither is left half-written on failure. Nothing was sent to anyone and nothing is waiting to be answered. Stado's own sentence below says what refused.",
            backendMessage: message
        )
    }

    /// Closing an offline invitation is the ordinary probing enrollment, so it
    /// fails in the ordinary ways. What it adds is where its two inputs came
    /// from: a fragment somebody else pasted, and an address somebody else
    /// typed into a message. Both are worth doubting before the network is.
    static func offlineClose(_ message: String, machine: String, sshTarget: String) -> Self {
        let inner = Self.enrollment(message, machine: machine, sshTarget: sshTarget)
        return Self(
            title: inner.title,
            detail: "\(inner.detail) The address \(sshTarget) was typed here from a message rather than reported by the machine, so read it again before anything else. If it is right, the fragment either was not pasted or was pasted into a different account than the one in that address — the key it appends lands in the home directory of whoever ran it.",
            backendMessage: message
        )
    }

    /// Adoption differs from the hand-installed key in exactly one way, and
    /// that one way is where it fails: Stado opens the first session itself,
    /// with whatever `ssh` on the control plane host can already authenticate
    /// with. The command's own sentence says whether the machine was not
    /// reached, refused the credential, or would not take the key; it is shown
    /// verbatim and not read back here for words.
    static func adoption(_ message: String, machine: String, sshTarget: String) -> Self {
        Self(
            title: "\(machine) was not adopted",
            detail: "Adoption opens a session to \(sshTarget) from the machine hosting the Stado control plane, which has no terminal, installs the public key, and probes the machine before anything is written. Nothing was written to the registry. Stado's own sentence below says which step refused.",
            backendMessage: message
        )
    }

    /// Approval runs the same probing enrollment as `fleet enroll`, so it has
    /// the same two failure modes and the same guarantee about what is left
    /// behind. What it adds is the address: it came from the machine, not from
    /// the operator, so a wrong one is a fact about the reply.
    static func approval(_ message: String, hostname: String, destination: String?) -> Self {
        let address = destination?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if address.isEmpty {
            return Self(
                title: "\(hostname) was not approved",
                detail: "Approval opens a channel to the machine and probes it before it writes anything, and this request carries no address to open one to. A machine that reported itself without a destination has to be enrolled with an address you supply.",
                backendMessage: message
            )
        }
        let inner = Self.enrollment(message, machine: hostname, sshTarget: address)
        return Self(
            title: inner.title,
            detail: "\(inner.detail) The address \(address) came from \(hostname) itself when it answered the invitation, so it is what that machine believes it is reachable at.",
            backendMessage: message
        )
    }

    static func rejection(_ message: String, hostname: String) -> Self {
        Self(
            title: "\(hostname) was not rejected",
            detail: "The request is still in the store waiting for a decision. Nothing was written to the registry either way.",
            backendMessage: message
        )
    }
}
