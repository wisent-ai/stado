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

    static func enrollment(_ message: String, machine: String, sshTarget: String) -> Self {
        let lowered = message.lowercased()
        if lowered.contains("rolled back") {
            return Self(
                title: "The agent install failed, so \(machine) was removed again",
                detail: "The registry entry was written, the agent install on the machine failed, and Stado rolled the entry back. There is no half-added machine to hunt for: the registry is exactly as it was before this attempt. Fix what the install complained about and enroll again.",
                backendMessage: message
            )
        }
        if lowered.contains("already registered") || lowered.contains("already has a health beacon") {
            return Self(
                title: "\(machine) is already in the registry",
                detail: "Enrollment refuses to overwrite a machine that already has a channel or a health beacon. Choose a different name, or work with the existing entry from the Hosts table.",
                backendMessage: message
            )
        }
        if lowered.contains("unsupported release platform") {
            return Self(
                title: "Stado reached \(sshTarget) but does not ship a release for it",
                detail: "The machine answered the identity probe with an operating system and architecture combination Stado has no release for, so no entry was written.",
                backendMessage: message
            )
        }
        return Self(
            title: "Stado could not reach \(sshTarget)",
            detail: "Enrollment asks the machine for its hostname, uname -s and uname -m before it writes anything, so this failure is about the connection and not about the registry. Nothing was written. Check that Remote Login is on over there, that the public key from the key step is in its ~/.ssh/authorized_keys, and that \(sshTarget) resolves from the machine running the Stado dashboard.",
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
        let lowered = message.lowercased()
        if lowered.contains("allow_invite") || lowered.contains("not allowed") || lowered.contains("refuses") {
            return Self(
                title: "This fleet's registry does not allow invitations",
                detail: "The catalog in the canonical registry switches this method off, and the preflight refused before anything was minted. No invitation exists and no key was created.",
                backendMessage: message
            )
        }
        return Self(
            title: "No invitation was minted for \(machine)",
            detail: "Minting writes one object to the store and one key pair to the credential store, in that order, and neither is left half-written on failure. Nothing was sent to anyone and nothing is waiting to be answered.",
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
    /// with. The command distinguishes three refusals — no connection, a
    /// rejected credential, and a home directory it could not write — and they
    /// send the operator to three different places.
    static func adoption(_ message: String, machine: String, sshTarget: String) -> Self {
        let lowered = message.lowercased()
        if lowered.contains("rejected the authentication") || lowered.contains("permission denied") {
            return Self(
                title: "\(sshTarget) answered, then refused the credentials",
                detail: "The machine is reachable, so this is about the credential and not the network. The key install runs from the machine hosting the Stado control plane, which has no terminal: OpenSSH there cannot prompt for a password, and no password can be supplied from this window. Either make the credential available to that host's SSH agent, or put an existing key of yours on \(sshTarget) — or use the invitation, which needs no credential from you at all. Nothing was written to the registry.",
                backendMessage: message
            )
        }
        if lowered.contains("no ssh connection") {
            return Self(
                title: "Nothing at \(sshTarget) answered on SSH",
                detail: "No session was established, so no credential was tried and nothing was written. Check that Remote Login is on over there and that \(sshTarget) resolves from the machine running the Stado control plane, which is where the connection is made from — not from this Mac.",
                backendMessage: message
            )
        }
        if lowered.contains("writing ~/.ssh/authorized_keys") {
            return Self(
                title: "\(sshTarget) let Stado in but would not take the key",
                detail: "The session opened and the credentials were accepted, and then writing the key into that account's ~/.ssh/authorized_keys failed. This is about the account on the machine: a read-only home directory, a full disk, or an authorized_keys file owned by somebody else. Nothing was written to the registry.",
                backendMessage: message
            )
        }
        if lowered.contains("allow_adopt") {
            return Self(
                title: "This fleet's registry does not allow Stado to install keys",
                detail: "The catalog switches adoption off, so the preflight refused before any session was opened. The invitation and the key installed by hand need no such permission, and either one is the way through.",
                backendMessage: message
            )
        }
        return .enrollment(message, machine: machine, sshTarget: sshTarget)
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
