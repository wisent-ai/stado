import Foundation
import SwiftUI

@MainActor
final class WorkloadAttachmentStore: ObservableObject {
    @Published private(set) var active = false
    @Published private(set) var connected = false
    @Published private(set) var inputClosed = false
    @Published private(set) var status = "Disconnected"
    @Published private(set) var standardOutput = ""
    @Published private(set) var standardError = ""
    @Published private(set) var problem: String?
    private var client: WorkloadStreamClient?
    private var generation = 0
    private var outputDecoder = StreamTextDecoder()
    private var errorDecoder = StreamTextDecoder()


    func connect(kind: String, target: String, workspace: String, resume: String,
                 fleet: FleetControlStore, expectedSource: Int) async {
        guard !active else { return }
        guard expectedSource == fleet.requestGeneration, let address = fleet.address else {
            problem = "The selected Stado endpoint changed. Review the attachment again."
            return
        }
        generation += 1
        let current = generation
        let client = WorkloadStreamClient()
        self.client = client
        active = true
        connected = false
        inputClosed = false
        standardOutput = ""
        standardError = ""
        outputDecoder = StreamTextDecoder()
        errorDecoder = StreamTextDecoder()
        problem = nil
        status = "Attaching to \(target)…"
        let session = resume.trimmingCharacters(in: .whitespacesAndNewlines)
        let request = WorkloadAttachmentRequest(kind: kind, target: target, workspace: workspace,
            resume: session.isEmpty ? nil : session, confirmation: "RUN_MUTATION")
        defer {
            if generation == current { active = false; connected = false }
        }
        do {
            try await client.connect(request, at: address, authorizationToken: fleet.authorizationToken)
            guard generation == current, expectedSource == fleet.requestGeneration else {
                await client.disconnect()
                return
            }
            while generation == current {
                let event = try await client.receive()
                guard generation == current, expectedSource == fleet.requestGeneration else { break }
                switch event {
                case .attached:
                    connected = true
                    status = "Stream connected to \(target)"
                case .stdout(let bytes): standardOutput += outputDecoder.append(bytes)
                case .stderr(let bytes): standardError += errorDecoder.append(bytes)
                case .exited(let code, let ok):
                    standardOutput += outputDecoder.finish()
                    standardError += errorDecoder.finish()
                    status = code.map { "Workload exited with status \($0)" } ?? "Workload ended without an exit code"
                    if !ok { problem = standardError.isEmpty ? status : standardError }
                    await client.disconnect()
                    return
                case .failure(let message):
                    problem = message
                    status = "Attachment failed"
                    await client.disconnect()
                    return
                }
            }
            await client.disconnect()
        } catch {
            guard generation == current else { return }
            problem = error.localizedDescription
            status = "Attachment failed"
            await client.disconnect()
        }
    }

    func send(_ text: String) async {
        guard connected, !inputClosed, let client else { return }
        let current = generation
        do { try await client.send(text.hasSuffix("\n") ? text : text + "\n") }
        catch { if generation == current { problem = error.localizedDescription } }
    }

    func finishInput() async {
        guard connected, !inputClosed, let client else { return }
        let current = generation
        do {
            try await client.finishInput()
            if generation == current { inputClosed = true }
        } catch { if generation == current { problem = error.localizedDescription } }
    }

    func disconnect() async {
        let connection = client
        client = nil
        generation += 1
        active = false
        connected = false
        status = "Disconnected"
        standardOutput += outputDecoder.finish()
        standardError += errorDecoder.finish()
        await connection?.disconnect()
    }
}

/// Keep an incomplete UTF-8 suffix between byte chunks. No complete output is
/// decoded again just because another chunk arrived.
private struct StreamTextDecoder {
    private var pending = Data()
    private static let maximumUTF8Suffix = 3

    mutating func append(_ bytes: Data) -> String {
        pending.append(bytes)
        for suffix in 0...min(Self.maximumUTF8Suffix, pending.count) {
            if let text = String(data: pending.dropLast(suffix), encoding: .utf8) {
                pending = Data(pending.suffix(suffix))
                return text
            }
        }
        return finish()
    }

    mutating func finish() -> String {
        let text = String(decoding: pending, as: UTF8.self)
        pending.removeAll(keepingCapacity: true)
        return text
    }
}
