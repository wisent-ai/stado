import Foundation

actor OperationsClient {
    let readSession: URLSession
    let convergenceSession: URLSession

    init(session: URLSession? = nil, convergenceSession: URLSession? = nil) {
        if let session {
            readSession = session
        } else {
            let configuration = Self.baseConfiguration()
            configuration.timeoutIntervalForRequest = 20
            configuration.timeoutIntervalForResource = 120
            readSession = URLSession(configuration: configuration)
        }

        if let convergenceSession {
            self.convergenceSession = convergenceSession
        } else if let session {
            self.convergenceSession = session
        } else {
            let configuration = Self.baseConfiguration()
            // A converge can legitimately spend 30 minutes in one product-owned
            // host archive stage. Its URLSession must therefore impose no
            // shorter transport deadline; the bounded product stages remain
            // the operation's deadlines. Read-only calls stay on readSession.
            configuration.timeoutIntervalForRequest = .greatestFiniteMagnitude
            configuration.timeoutIntervalForResource = .greatestFiniteMagnitude
            self.convergenceSession = URLSession(configuration: configuration)
        }
    }

    private static func baseConfiguration() -> URLSessionConfiguration {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.httpCookieStorage = nil
        configuration.httpShouldSetCookies = false
        configuration.urlCredentialStorage = nil
        return configuration
    }
}
