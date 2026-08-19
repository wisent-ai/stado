import Foundation
import XCTest
@testable import Stado

/// The fleet screen's decode and refusal paths, driven through a stubbed
/// HTTP bridge: the store must carry the CLI's sentences verbatim and never
/// invent a registry state the control plane did not return.
@MainActor
final class FleetGroupStoreTests: XCTestCase {
    func testDecodesTheFleetListEnvelopeWithMembersAndDefaults() throws {
        let list: FleetGroupList = try XCTUnwrap(
            FleetGroupStore.decode(
                from: """
                {"fleets": [
                    {"name": "build", "notes": "ci builders", "members": ["w1", "w2"]},
                    {"name": "edge"}
                ]}
                """
            )
        )
        XCTAssertEqual(list.fleets.count, 2)
        XCTAssertEqual(list.fleets[0].name, "build")
        XCTAssertEqual(list.fleets[0].notes, "ci builders")
        XCTAssertEqual(list.fleets[0].members, ["w1", "w2"])
        XCTAssertEqual(list.fleets[1].members, [], "absent members decode empty, not crash")
        XCTAssertEqual(list.fleets[1].notes, "")
    }

    func testARefusedDeleteArrivesInTheCLIsOwnSentence() async throws {
        let refusal = "fleet 'build' still has 1 member(s): w1; reassign them first"
        let store = FleetGroupStore(
            client: FleetControlClient(session: Self.stubbedSession { request in
                XCTAssertEqual(request.url?.path, "/api/operator/run")
                let body = try XCTUnwrap(Self.bodyData(of: request))
                let args = try XCTUnwrap(
                    (JSONSerialization.jsonObject(with: body) as? [String: Any])?["args"]
                        as? [String]
                )
                XCTAssertEqual(args, ["fleet", "delete", "build"])
                return Self.bridge(ok: false, stderr: "Error: \(refusal)")
            })
        )
        store.configureEndpoint("http://127.0.0.1:8765")

        await store.delete(name: "build")

        guard case let .failed(message) = store.mutation else {
            return XCTFail("a refused delete must fail the mutation, got \(store.mutation)")
        }
        XCTAssertTrue(
            message.contains(refusal),
            "the refusal is the CLI's sentence, not a paraphrase: \(message)"
        )
    }

    func testAReadFailureNamesTheBackendSentence() async throws {
        let store = FleetGroupStore(
            client: FleetControlClient(session: Self.stubbedSession { _ in
                Self.bridge(ok: false, stderr: "Error: no registry document at local:registry.json")
            })
        )
        store.configureEndpoint("http://127.0.0.1:8765")

        await store.refresh()

        XCTAssertEqual(store.fleets, [])
        XCTAssertEqual(store.failure, "Error: no registry document at local:registry.json")
    }

    // MARK: Stub

    /// URLSession hands a request body to a URLProtocol as a stream, never as
    /// `httpBody`; read it fully.
    private static func bodyData(of request: URLRequest) -> Data? {
        if let body = request.httpBody { return body }
        guard let stream = request.httpBodyStream else { return nil }
        stream.open()
        defer { stream.close() }
        var data = Data()
        let buffer = UnsafeMutablePointer<UInt8>.allocate(capacity: 4096)
        defer { buffer.deallocate() }
        while stream.hasBytesAvailable {
            let read = stream.read(buffer, maxLength: 4096)
            if read <= 0 { break }
            data.append(buffer, count: read)
        }
        return data
    }

    private static func bridge(ok: Bool, stderr: String) -> (HTTPURLResponse, Data) {
        let payload = try! JSONSerialization.data(withJSONObject: [
            "ok": ok,
            "exit_code": ok ? 0 : 1,
            "stdout": "",
            "stderr": stderr,
        ])
        let response = HTTPURLResponse(
            url: URL(string: "http://127.0.0.1:8765/api/operator/run")!,
            statusCode: 200,
            httpVersion: nil,
            headerFields: nil
        )!
        return (response, payload)
    }

    private static func stubbedSession(
        answer: @escaping (URLRequest) throws -> (HTTPURLResponse, Data)
    ) -> URLSession {
        StubURLProtocol.answer = answer
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [StubURLProtocol.self]
        return URLSession(configuration: configuration)
    }
}

private final class StubURLProtocol: URLProtocol, @unchecked Sendable {
    nonisolated(unsafe) static var answer: ((URLRequest) throws -> (HTTPURLResponse, Data))?

    override class func canInit(with request: URLRequest) -> Bool { true }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        guard let answer = Self.answer else {
            client?.urlProtocol(self, didFailWithError: URLError(.unknown))
            return
        }
        do {
            let (response, data) = try answer(request)
            client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: data)
            client?.urlProtocolDidFinishLoading(self)
        } catch {
            client?.urlProtocol(self, didFailWithError: error)
        }
    }

    override func stopLoading() {}
}
