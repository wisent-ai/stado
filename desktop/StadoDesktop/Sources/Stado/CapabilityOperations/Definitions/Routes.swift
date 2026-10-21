import Foundation

enum NativeRouteOperations {
    static let all: [NativeCapabilityOperation] = [
        .init(id: "resolver-status", title: "Read this host's resolver: state, adapters, authority, blockers and the stops of a stado serve build that stops on every start", path: ["resolver", "status"], hostPlacement: .option("--target"), mutates: false),
        .init(id: "resolver-channels", title: "Read local resolver channel publication (no health probe)", path: ["resolver", "channels"], hostPlacement: .none, fields: [
            .init(id: "bind", label: "Exact adapter host:port on the selected API host (optional)", option: "--bind"),
            .init(id: "consumer", label: "Consumer identity (optional)", option: "--consumer"),
        ], mutates: false),
        // The selected host's recorded service recovery attempts, including
        // refused repair steps and restarts. Each record names the host the
        // service runs on and, for a peer attempt, the observing host.
        .init(id: "service-recoveries", title: "Read this host's service recovery attempts, including refusals", path: ["service", "recoveries"], hostPlacement: .none, fields: [
            .init(id: "service", label: "Only this service (blank reads every recovery attempt)", option: "--service"),
        ], mutates: false),
        .init(id: "directory-connect", title: "Resolve a consumer's service route and diagnose its connection", path: ["service", "directory", "connect"], hostPlacement: .none, fields: [
            .init(id: "service", label: "Declared service", required: true),
            .init(id: "consumer", label: "Consumer identity", option: "--consumer", required: true),
            .init(id: "target", label: "Caller host (blank uses the selected API host; other hosts are not probed)", option: "--target"),
            .init(id: "no-verify", label: "Read routing declaration only, without proving a connection", option: "--no-verify", flag: true),
        ], mutates: false),
        .init(id: "directory-doctor", title: "Diagnose this host's admitted service connection", path: ["service", "directory", "doctor"], hostPlacement: .none, fields: [
            .init(id: "service", label: "Declared service", required: true),
            .init(id: "consumer", label: "Consumer identity", option: "--consumer", required: true),
        ], mutates: false),
        .init(id: "consumer-add", title: "Declare or update a consumer and its route", path: ["service", "directory", "consumer", "add"], hostPlacement: .none, fields: [
            .init(id: "service", label: "Service", required: true),
            .init(id: "consumer", label: "Consumer", required: true),
            .init(id: "capability", label: "Capabilities (one per line; blank preserves existing)", option: "--capability", multiple: true),
            .init(id: "target", label: "Resolver host (it hands out the loopback port)", option: "--target"),
            .init(id: "reassign", label: "Give the existing adapter on that host a new port from the host", option: "--reassign", flag: true),
        ]),
        .init(id: "resolver-api-reassign", title: "Give this host's resolution API a port the host hands out", path: ["resolver", "api-reassign"], hostPlacement: .option("--target")),
        .init(id: "consumer-remove", title: "Remove a consumer and all its resolver bindings", path: ["service", "directory", "consumer", "remove"], hostPlacement: .none, fields: [
            .init(id: "service", label: "Service", required: true),
            .init(id: "consumer", label: "Consumer to remove from every host", required: true),
        ]),
        .init(id: "beside-list", title: "Read the services this host runs beside their placement", path: ["service", "directory", "beside"], hostPlacement: .none, mutates: false),
        .init(id: "beside-declare", title: "Declare a service this host runs for its own callers beside its placement", path: ["service", "directory", "beside"], hostPlacement: .none, fields: [
            .init(id: "service", label: "Service", required: true),
            .init(id: "url", label: "Loopback URL of this host's own instance, with its port", option: "--url", required: true),
        ]),
        .init(id: "beside-withdraw", title: "Take back a service this host declared beside its placement", path: ["service", "directory", "beside"], hostPlacement: .none, fields: [
            .init(id: "service", label: "Service", required: true),
            .init(id: "withdraw", label: "Withdraw it", option: "--withdraw", flag: true),
        ]),
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
        .init(id: "placement-move", title: "Move a placement profile to another declared host", path: ["placement", "move"], hostPlacement: .none, fields: [
            .init(id: "services", label: "Logical services naming one placement profile (one per line)", required: true, multiple: true),
            .init(id: "to_host", label: "Destination host declared by the profile", option: "--to-host", required: true),
        ]),
        .init(id: "placement-take-authority", title: "Take the registry authority role on this API host while the authority does not answer", path: ["placement", "move"], hostPlacement: .none, fields: [
            .init(id: "to_host", label: "This API host's registry target (the role is taken where the command runs)", option: "--to-host", required: true),
        ], fixedArguments: ["--authority"]),
        .init(id: "placement-standby", title: "Prepare a registered host to stand by for a placement profile", path: ["placement", "standby"], hostPlacement: .option("--host"), fields: [
            .init(id: "profile", label: "Placement profile", required: true),
            .init(id: "reason", label: "Why this host must stand by (carried in the receipt; each unit is staged, not run)", option: "--reason", required: true),
        ]),
        .init(id: "placement-profile-list", title: "List the placement profiles the registry holds", path: ["placement", "profile", "list"], hostPlacement: .none, mutates: false),
        .init(id: "placement-profile-show", title: "Show one placement profile as the registry holds it", path: ["placement", "profile", "show"], hostPlacement: .none, fields: [
            .init(id: "name", label: "Placement profile", required: true),
        ], mutates: false, jsonOutput: false),
        .init(id: "placement-profile-declare", title: "Declare or replace a placement profile and link its services in the directory", path: ["placement", "profile", "declare"], hostPlacement: .none, fields: [
            .init(id: "file", label: "Profile JSON file on the selected API host", option: "--file", required: true),
        ]),
        .init(id: "placement-profile-remove", title: "Remove a placement profile; its services become fixed routes on their active hosts", path: ["placement", "profile", "remove"], hostPlacement: .none, fields: [
            .init(id: "name", label: "Placement profile", required: true),
        ]),
    ]
}
