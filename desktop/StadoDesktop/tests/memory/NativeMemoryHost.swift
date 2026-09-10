import CryptoKit
import Darwin
import Foundation
import XCTest

/// Real binaries, a private registry and an encrypted vault. No HTTP doubles.
@MainActor
final class NativeMemoryHost {
    let name = "memory-native"
    let root: URL
    let registry: URL
    let endpoint: String
    private let stado: URL
    private let skarbiec: URL
    private var environment: [String: String]
    private var children: [Process] = []
    private var handles: [FileHandle] = []
    private var previousEnvironment: [String: String] = [:]
    private var changedEnvironment: [String] = []
    private var sequence = 0
    private var stopped = false

    init() throws {
        let caller = ProcessInfo.processInfo.environment
        let package = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let repo = package.deletingLastPathComponent().deletingLastPathComponent()
        stado = URL(fileURLWithPath: caller["STADO_BIN"] ?? repo.appendingPathComponent("stado-rs/target/debug/stado").path)
        skarbiec = URL(fileURLWithPath: try XCTUnwrap(caller["SKARBIEC_BIN"], "Set SKARBIEC_BIN to the real consolidated broker"))
        let revision = try XCTUnwrap(caller["STADO_SOURCE_REVISION"], "Set STADO_SOURCE_REVISION to the compiled revision")
        let runID = String(UUID().uuidString.prefix(8))
        root = package.appendingPathComponent(".build/test-results/memory-\(runID)")
        let storage = root.appendingPathComponent("storage")
        registry = storage.appendingPathComponent("registry.json")
        endpoint = "http://127.0.0.1:\(try Self.availablePort())"
        // Only the keyring needs the short root: macOS limits Unix socket paths to 104 bytes.
        let gnupg = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".stado/test-runs/mm-\(runID)")
        environment = ["HOME": root.path, "GNUPGHOME": gnupg.path,
            "TMPDIR": root.appendingPathComponent("temporary").path,
            "PATH": caller["PATH"] ?? "/usr/bin:/bin:/usr/sbin:/sbin",
            "SKARBIEC_VAULT_FILE": root.appendingPathComponent("vault.json").path,
            "SKARBIEC_AUDIT_FILE": root.appendingPathComponent("vault.audit.jsonl").path,
            "STADO_CONFIG": root.appendingPathComponent("config.json").path,
            "WC_STORAGE_BACKEND": "local", "WC_LOCAL_STORAGE_PATH": storage.path,
            "WC_PROVIDERS": "local", "NO_COLOR": "1"]
        for directory in [root, storage, gnupg, root.appendingPathComponent("temporary")] {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true,
                attributes: [.posixPermissions: 0o700])
        }
        do {
            try record(["revision": revision, "binaries": [try identity(stado), try identity(skarbiec)]], named: "source.json")
            // The role is part of the declaration a policy is written for:
            // without it no declared policy fits this fixture and the catalog
            // path could not be exercised at all.
            let target: [String: Any] = ["name": name, "kind": "local", "ssh": NSNull(),
                "release_platform": "darwin-arm64", "role": "always-on",
                "hostnames": [ProcessInfo.processInfo.hostName], "services": []]
            try JSONSerialization.data(withJSONObject: ["schema_version": 2, "targets": [target], "coordinators": []])
                .write(to: registry)
            try record(["storage": ["backend": "local", "local": ["path": storage.path]]], named: "config.json")
            _ = try run(skarbiec, ["version"])
            _ = try run(stado, ["--version"])
            _ = try run(skarbiec, ["init", "Native Memory Test <memory@example.invalid>"])
            let item = "memory-desktop-registry-api"
            let bearer = UUID().uuidString
            _ = try run(skarbiec, ["set", item, "--type", "token", "token=\(bearer)"])
            let grant = try run(skarbiec, ["grant", "issue", "stado-registry-api-verifier", "--capabilities", "read:\(item)#token"])
            let token = try XCTUnwrap(grant["token"] as? String)
            let verifierFile = root.appendingPathComponent("verifier-token")
            let desktopFile = root.appendingPathComponent("desktop-token")
            try privateFile(token, at: verifierFile)
            try privateFile(bearer, at: desktopFile)
            let vaultPort = try Self.availablePort()
            environment["WC_REGISTRY_SKARBIEC_URL"] = "http://127.0.0.1:\(vaultPort)"
            environment["WC_REGISTRY_SKARBIEC_TOKEN_FILE"] = verifierFile.path
            environment["WC_REGISTRY_API_CLIENTS"] = "{\"memory-desktop\":{\"item\":\"\(item)\",\"actions\":[\"policy-read\",\"policy-write\"]}}"
            try start(skarbiec, ["serve", "--port", String(vaultPort)], named: "skarbiec")
            try start(stado, ["dashboard", "--bind", "127.0.0.1", "--port", URL(string: endpoint)!.port!.description], named: "stado")
            for (key, value) in ["STADO_REGISTRY_API_URL": endpoint, "STADO_REGISTRY_API_TOKEN_FILE": desktopFile.path] {
                previousEnvironment[key] = caller[key]
                changedEnvironment.append(key)
                setenv(key, value, 1)
            }
            print("Memory native API evidence: \(root.path)")
        } catch {
            stop()
            throw error
        }
    }

    func waitUntilListening() async throws {
        let deadline = Date().addingTimeInterval(30)
        while Date() < deadline, children.allSatisfy(\.isRunning) {
            if let (_, response) = try? await URLSession.shared.data(from: URL(string: endpoint + "/healthz")!),
               let response = response as? HTTPURLResponse, response.statusCode == 200 { return }
            try await Task.sleep(for: .milliseconds(100))
        }
        throw NSError(domain: "MemoryTests", code: 1,
            userInfo: [NSLocalizedDescriptionKey: "The real API did not start; logs: \(root.path)"])
    }

    func policy() throws -> [String: Any] {
        let document = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: registry)) as? [String: Any])
        let targets = try XCTUnwrap(document["targets"] as? [[String: Any]])
        let target = try XCTUnwrap(targets.first { $0["name"] as? String == name })
        return target["memory_reclaim"] as? [String: Any] ?? [:]
    }

    func record(_ value: [String: Any], named name: String) throws {
        try JSONSerialization.data(withJSONObject: value, options: [.prettyPrinted, .sortedKeys])
            .write(to: root.appendingPathComponent(name))
    }

    func stop() {
        guard !stopped else { return }
        stopped = true
        for child in children.reversed() where child.isRunning { child.terminate(); child.waitUntilExit() }
        children.removeAll()
        for handle in handles { try? handle.close() }
        handles.removeAll()
        for key in changedEnvironment {
            if let previous = previousEnvironment[key] { setenv(key, previous, 1) } else { unsetenv(key) }
        }
        changedEnvironment.removeAll()
        // The test owns this GPG home, and no other home is passed to gpgconf.
        let cleanup = Process()
        cleanup.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        cleanup.arguments = ["gpgconf", "--kill", "all"]
        cleanup.environment = environment
        cleanup.standardOutput = FileHandle.nullDevice
        cleanup.standardError = FileHandle.nullDevice
        if (try? cleanup.run()) != nil { cleanup.waitUntilExit() }
        if let keyring = environment["GNUPGHOME"] { try? FileManager.default.removeItem(atPath: keyring) }
    }

    private func identity(_ binary: URL) throws -> [String: String] {
        let digest = SHA256.hash(data: try Data(contentsOf: binary, options: .mappedIfSafe))
            .map { String(format: "%02x", $0) }.joined()
        return ["path": binary.path, "sha256": digest]
    }

    private func privateFile(_ value: String, at path: URL) throws {
        try Data(value.utf8).write(to: path)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: path.path)
    }

    private func log(_ name: String) throws -> FileHandle {
        let path = root.appendingPathComponent(name)
        _ = FileManager.default.createFile(atPath: path.path, contents: nil)
        let handle = try FileHandle(forWritingTo: path)
        handles.append(handle)
        return handle
    }

    private func start(_ binary: URL, _ arguments: [String], named name: String) throws {
        let child = Process()
        child.executableURL = binary
        child.arguments = arguments
        child.environment = environment
        child.standardOutput = try log(name + ".stdout")
        child.standardError = try log(name + ".stderr")
        try child.run()
        children.append(child)
    }

    private func run(_ binary: URL, _ arguments: [String]) throws -> [String: Any] {
        sequence += 1
        let name = "command-\(sequence)"
        let child = Process()
        child.executableURL = binary
        child.arguments = arguments
        child.environment = environment
        child.standardOutput = try log(name + ".stdout")
        child.standardError = try log(name + ".stderr")
        try child.run()
        child.waitUntilExit()
        let output = try Data(contentsOf: root.appendingPathComponent(name + ".stdout"))
        let error = try String(contentsOf: root.appendingPathComponent(name + ".stderr"), encoding: .utf8)
        try record(["binary": binary.path, "arguments": arguments, "exit_code": child.terminationStatus], named: name + ".json")
        guard child.terminationStatus == 0 else {
            throw NSError(domain: "MemoryTests", code: Int(child.terminationStatus),
                userInfo: [NSLocalizedDescriptionKey: "\(arguments.joined(separator: " ")): \(error); evidence: \(root.path)"])
        }
        return (try? JSONSerialization.jsonObject(with: output) as? [String: Any]) ?? [:]
    }

    private static func availablePort() throws -> UInt16 {
        let fd = socket(AF_INET, SOCK_STREAM, 0)
        guard fd >= 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
        defer { close(fd) }
        var address = sockaddr_in()
        address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        address.sin_family = sa_family_t(AF_INET)
        address.sin_addr.s_addr = inet_addr("127.0.0.1")
        var size = socklen_t(MemoryLayout<sockaddr_in>.size)
        let result = withUnsafeMutablePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.bind(fd, $0, size) == 0 ? getsockname(fd, $0, &size) : -1
            }
        }
        guard result == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
        return UInt16(bigEndian: address.sin_port)
    }
}
