import Foundation

/// Reading one host's retained Tailscale messages through the authenticated
/// native argv bridge, and deciding whether the answer can be believed.
extension FleetControlStore {
    nonisolated static func tailscaleLogArguments(
        host: String,
        source: HostTailscaleLogSource
    ) -> [String] {
        ["host", "exec", host, "--json", "--"] + source.command
    }

    func tailscaleLogAttempt(for host: String) -> HostTailscaleLogAttempt? {
        tailscaleLogAttempts[host]
    }

    func isReadingTailscaleLogs(from host: String) -> Bool {
        tailscaleLogReadingHosts.contains(host)
    }

    /// Read the selected host's retained Tailscale messages through Stado's
    /// authenticated native argv bridge. The source is required from the UI:
    /// neither the registry projection nor the capacity report declares an OS.
    func readTailscaleLogs(host: String, source: HostTailscaleLogSource) async {
        guard !host.isEmpty, !tailscaleLogReadingHosts.contains(host) else { return }
        let arguments = Self.tailscaleLogArguments(host: host, source: source)
        guard let address else {
            tailscaleLogAttempts[host] = HostTailscaleLogAttempt(
                requestedHost: host,
                source: source,
                arguments: arguments,
                completedAt: Date(),
                result: nil,
                receipt: nil,
                failure: "No Stado endpoint is configured, so the retained Tailscale logs were not read."
            )
            return
        }

        let generation = requestGeneration
        tailscaleLogReadingHosts.insert(host)
        defer {
            if requestGeneration == generation {
                tailscaleLogReadingHosts.remove(host)
            }
        }
        do {
            let result = try await client.run(
                arguments: arguments,
                confirmsMutation: false,
                at: address,
                authorizationToken: authorizationToken
            )
            guard requestGeneration == generation, !Task.isCancelled else { return }
            let receipt = try? JSONDecoder().decode(
                HostTailscaleLogReceipt.self,
                from: Data(result.standardOutput.utf8)
            )
            tailscaleLogAttempts[host] = HostTailscaleLogAttempt(
                requestedHost: host,
                source: source,
                arguments: arguments,
                completedAt: Date(),
                result: result,
                receipt: receipt,
                failure: Self.tailscaleLogFailure(
                    result: result,
                    receipt: receipt,
                    requestedHost: host,
                    source: source
                )
            )
        } catch is CancellationError {
            return
        } catch let error as URLError where error.code == .cancelled {
            return
        } catch {
            guard requestGeneration == generation else { return }
            tailscaleLogAttempts[host] = HostTailscaleLogAttempt(
                requestedHost: host,
                source: source,
                arguments: arguments,
                completedAt: Date(),
                result: nil,
                receipt: nil,
                failure: Self.describe(error)
            )
        }
    }

    private nonisolated static func tailscaleLogFailure(
        result: OperatorCommandResult,
        receipt: HostTailscaleLogReceipt?,
        requestedHost: String,
        source: HostTailscaleLogSource
    ) -> String? {
        guard let receipt else {
            if let refusal = nonEmpty(result.standardError) {
                return refusal
            }
            return result.ok
                ? "Stado returned a host-exec response that Desktop could not read."
                : result.message
        }
        guard receipt.schema == "stado.host-exec-receipt.v1" else {
            return "Stado returned host-exec receipt schema \(receipt.schema), not stado.host-exec-receipt.v1."
        }
        guard receipt.target == requestedHost else {
            return "The retained-log request named \(requestedHost), but the host-exec receipt names \(receipt.target)."
        }
        guard result.arguments == tailscaleLogArguments(host: requestedHost, source: source) else {
            return "The Stado API receipt did not report the retained-log invocation that Desktop requested."
        }
        guard receipt.command == source.command.joined(separator: " "),
              receipt.arguments == source.receiptArguments
        else {
            return "The host-exec receipt did not report the fixed \(source.title) command that Desktop requested."
        }
        guard result.readOnly else {
            return "Stado did not classify this host-exec operation as read-only."
        }
        guard result.ok, receipt.status == "ok" else {
            return nonEmpty(result.standardError)
                ?? receipt.error.flatMap(nonEmpty)
                ?? nonEmpty(receipt.standardError)
                ?? "The retained-log command exited with code \(receipt.exitCode) and printed no error."
        }
        return nil
    }

    private nonisolated static func nonEmpty(_ value: String) -> String? {
        value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? nil : value
    }
}
