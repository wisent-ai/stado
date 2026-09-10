import Foundation

enum NativeRouteOperations {
    static let all: [NativeCapabilityOperation] = [
        .init(id: "open", title: "Open a declared service route", path: ["route", "open"], hostPlacement: .none, fields: [
            .init(id: "service", label: "Service", required: true),
            .init(id: "target", label: "Endpoint holder (blank uses active host)", option: "--target"),
            .init(id: "direction", label: "Local API account or remote endpoint holder", required: true, choices: ["--local", "--remote"]),
        ]),
        .init(id: "close", title: "Close the service's current route", path: ["route", "close"], hostPlacement: .none, fields: [
            .init(id: "service", label: "Service", required: true),
            .init(id: "target", label: "Remote endpoint holder (local marker is checked first)", option: "--target"),
        ], jsonOutput: false),
        .init(id: "capability", title: "Read service credential routes", path: ["route", "capability"], hostPlacement: .none, fields: [
            .init(id: "service", label: "Service", required: true),
        ], mutates: false),
        .init(id: "key", title: "Authorize this host's resolver public key", path: ["route", "key"], hostPlacement: .positional),
        .init(id: "placement", title: "Publish declared fleet placement policies", path: ["route", "placement", "publish"], hostPlacement: .none, fields: [
            .init(id: "mobile", label: "Only active hosts declaring mobile runtime", option: "--mobile", flag: true),
        ]),
    ]
}
