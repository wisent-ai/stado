import Foundation

/// How the operator is asked: the same `stado alerts` verbs as the CLI. The
/// choice is the fleet's (`operator_contact` in the registry), so no host is
/// placed on the command line.
enum NativeOperatorContactOperations {
    static let channels = ["slack", "telegram", "sendgrid", "resend", "most", "gcp-pubsub"]

    static let all: [NativeCapabilityOperation] = [
        .init(id: "preferences-show", title: "Read how the operator is asked", path: ["alerts", "preferences", "show"],
              hostPlacement: .none, fixedArguments: ["--json"], mutates: false),
        .init(id: "preferences-set", title: "Choose how the operator is asked (first preferred first)",
              path: ["alerts", "preferences", "set"], hostPlacement: .none, fields: [
                  .init(id: "channel", label: "Channels, one per line, first preferred first (\(channels.joined(separator: ", ")))",
                        option: "--channel", required: true, multiple: true),
              ], jsonOutput: false),
        .init(id: "channels", title: "Read where each chosen channel delivers", path: ["alerts", "channels"],
              hostPlacement: .none, fixedArguments: ["--json"], mutates: false),
        .init(id: "send", title: "Send one test message through the chosen channels", path: ["alerts", "send"],
              hostPlacement: .none, fields: [
                  .init(id: "message", label: "Message", required: true),
              ], jsonOutput: false),
    ]
}
