import Foundation

/// Every group of capability operations the console offers, in one list, so
/// the CLI paths they run can be listed and checked against the CLI itself.
/// An operation naming a verb the CLI does not have used to reach the
/// operator as a bare usage error; `Stado --operation-paths` prints every
/// path, and `tests/desktop_api` runs each one's `--help` through the built
/// `stado`.
enum NativeOperationCatalog {
    static let groups: [(name: String, operations: [NativeCapabilityOperation])] = [
        ("routes", NativeRouteOperations.all),
        ("network", NativeNetworkOperations.all),
        ("space", NativeSpaceOperations.all),
        ("host-release", NativeHostReleaseOperations.all),
        ("release-source", NativeReleaseSourceOperations.all),
        ("products", NativeProductOperations.all),
        ("queue", NativeQueueOperations.all),
        ("change-queue", NativeChangeQueueOperations.all),
        ("credentials", NativeCredentialOperations.all),
        ("registry", NativeRegistryOperations.all),
        ("operator-contact", NativeOperatorContactOperations.all),
        ("compute-providers", NativeComputeProviderOperations.all),
        ("fleet-sandboxes", NativeFleetSandboxOperations.all),
        ("sandboxes", NativeSandboxOperations.all),
        ("sandbox-accounts", NativeSandboxAccountOperations.all),
    ]

    /// With `--operation-paths` on the command line: print every operation's
    /// group, id, CLI path, fixed arguments and the options its fields pass
    /// as one JSON array and exit, before any window opens. Without it:
    /// nothing.
    static func printPathsIfAsked() {
        guard CommandLine.arguments.contains("--operation-paths") else { return }
        let operations: [[String: Any]] = groups.flatMap { group in
            group.operations.map { operation in
                [
                    "group": group.name,
                    "id": operation.id,
                    "path": operation.path,
                    "fixed_arguments": operation.fixedArguments,
                    "options": operation.fields.compactMap(\.option),
                ]
            }
        }
        let rows = operations
        do {
            let data = try JSONSerialization.data(withJSONObject: rows, options: [.prettyPrinted, .sortedKeys])
            FileHandle.standardOutput.write(data)
            FileHandle.standardOutput.write(Data("\n".utf8))
            exit(EXIT_SUCCESS)
        } catch {
            FileHandle.standardError.write(Data("the operation paths cannot be written as JSON: \(error)\n".utf8))
            exit(EXIT_FAILURE)
        }
    }
}
