import Foundation

enum NativeRouteOperations {
    static let all: [NativeCapabilityOperation] = [
        .init(id: "resolver-status", title: "Read this host's resolver: state, adapters, authority and what this host remembers of reading it, blockers and the stops of a stado serve build that stops on every start", path: ["resolver", "status"], hostPlacement: .option("--target"), mutates: false),
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
        .init(id: "resolver-api-reassign", title: "Give this host's resolution API a port the host hands out", path: ["resolver", "api", "reassign"], hostPlacement: .option("--target")),
        // What this host remembers about reading its remote registry
        // authority, and dropping it once the authority or the network is
        // repaired, so the next command asks at once.
        .init(id: "resolver-remembered-show", title: "Read what this host remembers about reading its registry authority: a read that got no snapshot, and the kept snapshot", path: ["resolver", "remembered", "show"], hostPlacement: .none, mutates: false),
        .init(id: "resolver-remembered-forget", title: "Forget both, so the next command asks the registry authority at once", path: ["resolver", "remembered", "forget"], hostPlacement: .none),
        // This host's own registry copy, readable while the authority and
        // the store are down: the document readers here fall back to.
        .init(id: "registry-last-good-read", title: "Read this host's last-known-good registry copy, which names the authority it asks", path: ["registry", "pull"], hostPlacement: .none, fixedArguments: ["--last-known-good"], mutates: false, jsonOutput: false),
        // The operator's acceptance of one directory generation the authority
        // answers below the one this host's resolver holds, as its rollback
        // refusal in resolver status names it.
        .init(id: "resolver-accept", title: "Accept the lower directory generation the authority answers, after reading both store versions in resolver status", path: ["resolver", "accept"], hostPlacement: .none, fields: [
            .init(id: "generation", label: "The generation the authority answers, as the rollback refusal names it", option: "--generation", required: true),
        ]),
        .init(id: "resolver-follow", title: "Ask this API host's resolver to read the registry now and wait until it holds the authority's generation, or the one given", path: ["resolver", "follow"], hostPlacement: .none, fields: [
            .init(id: "generation", label: "Directory generation it must hold (blank: the one the authority serves)", option: "--generation"),
        ], mutates: false),
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
        .init(id: "placement-move-check", title: "Check whether a placement move would pass its preflight, claiming and changing nothing", path: ["placement", "move"], hostPlacement: .none, fields: [
            .init(id: "services", label: "Logical services naming one placement profile (one per line)", required: true, multiple: true),
            .init(id: "to_host", label: "Destination host declared by the profile", option: "--to-host", required: true),
        ], fixedArguments: ["--check"], mutates: false),
        .init(id: "placement-evacuate-check", title: "Check whether one host could be emptied onto another (install roots, vault, profiles, host Stado roles, authority), changing nothing", path: ["placement", "evacuate"], hostPlacement: .none, fields: [
            .init(id: "from_host", label: "Host everything leaves", option: "--from-host", required: true),
            .init(id: "to_host", label: "Host everything moves to", option: "--to-host", required: true),
            .init(id: "reason", label: "Why", option: "--reason", required: true),
            .init(id: "vault_owner_uid", label: "Vault owner the destination's keyring holds (when the vault moves)", option: "--vault-owner-uid"),
            .init(id: "authority", label: "Take the registry authority last (run on the destination)", option: "--authority", flag: true),
            .init(id: "standby", label: "Run standby's check for the profiles the destination does not fully stand by for", option: "--standby", flag: true),
            .init(id: "profile_fixed_routes", label: "Draft a placement profile for each fixed route on the source (nothing declared)", option: "--profile-fixed-routes", flag: true),
            .init(id: "repin_schedules", label: "List the schedules pinned to the source that the run would repin", option: "--repin-schedules", flag: true),
            .init(id: "drop_release_targets", label: "List the release-control products the run would stop releasing to the source", option: "--drop-release-targets", flag: true),
            .init(id: "portable_roots", label: "List the install roots the run would write under {home} for the destination", option: "--portable-roots", flag: true),
        ], fixedArguments: ["--check"], mutates: false),
        .init(id: "placement-evacuate", title: "Empty one host onto another: install roots, vault, profiles, host Stado roles, and the authority, after checking every step", path: ["placement", "evacuate"], hostPlacement: .none, fields: [
            .init(id: "from_host", label: "Host everything leaves", option: "--from-host", required: true),
            .init(id: "to_host", label: "Host everything moves to", option: "--to-host", required: true),
            .init(id: "reason", label: "Why (recorded with each role's write fence)", option: "--reason", required: true),
            .init(id: "vault_owner_uid", label: "Vault owner the destination's keyring holds (when the vault moves)", option: "--vault-owner-uid"),
            .init(id: "authority", label: "Take the registry authority last (run on the destination)", option: "--authority", flag: true),
            .init(id: "standby", label: "Stand the destination by for every profile it does not fully stand by for", option: "--standby", flag: true),
            .init(id: "profile_fixed_routes", label: "Give each fixed route on the source a placement profile of its own first, so it moves too", option: "--profile-fixed-routes", flag: true),
            .init(id: "repin_schedules", label: "Pin the schedules pinned to the source to the destination once it is emptied", option: "--repin-schedules", flag: true),
            .init(id: "drop_release_targets", label: "Stop releasing to the source each release-control product that rolls out there, once it is emptied", option: "--drop-release-targets", flag: true),
            .init(id: "portable_roots", label: "Write each install root that would send the destination outside its home under {home} first", option: "--portable-roots", flag: true),
        ]),
        .init(id: "placement-move-role", title: "Move a role of the host Stado process (the fleet object API) to another host: fence its store, copy it, switch the route", path: ["placement", "role", "move"], hostPlacement: .none, fields: [
            .init(id: "service", label: "Directory service the host Stado process serves", required: true),
            .init(id: "to_host", label: "Host it moves to", option: "--to-host", required: true),
            .init(id: "reason", label: "Why it moves (recorded with the source's write fence)", option: "--reason", required: true),
            .init(id: "with_authority", label: "Carry the registry authority in the same registry (required when the source holds it; run on the destination)", option: "--with-authority", flag: true),
        ]),
        .init(id: "placement-take-authority-check", title: "Check whether this API host could take the registry authority role, and how, writing nothing", path: ["placement", "move"], hostPlacement: .none, fields: [
            .init(id: "to_host", label: "This API host's registry target", option: "--to-host", required: true),
        ], fixedArguments: ["--authority", "--check"], mutates: false),
        .init(id: "placement-history", title: "Every placement change this API host ran, newest first, with its receipt or refusal", path: ["placement", "history"], hostPlacement: .none, mutates: false),
        .init(id: "placement-holds", title: "What the registry places on this host: profiles (the vault's marked), host Stado roles, the registry authority, fixed routes, release-control products, pinned schedules and its store's write fence; whether it is emptied", path: ["placement", "holds"], hostPlacement: .positional, mutates: false),
        .init(id: "placement-holds-everywhere", title: "Where every placed service runs: per host, its profiles, host Stado roles, the registry authority, fixed routes, release-control products, pinned schedules and its store's write fence, and whether it is emptied", path: ["placement", "holds"], hostPlacement: .none, mutates: false),
        .init(id: "placement-move-role-check", title: "Check whether a role of the host Stado process could move: its route, and room for its store", path: ["placement", "role", "move"], hostPlacement: .none, fields: [
            .init(id: "service", label: "Directory service the host Stado process serves", required: true),
            .init(id: "to_host", label: "Host it would move to", option: "--to-host", required: true),
            .init(id: "reason", label: "Why it would move", option: "--reason", required: true),
            .init(id: "with_authority", label: "Check it carrying the registry authority", option: "--with-authority", flag: true),
        ], fixedArguments: ["--check"], mutates: false),
        .init(id: "placement-take-authority", title: "Move the registry authority role to this API host: handed over by an authority that answers, taken while it does not", path: ["placement", "move"], hostPlacement: .none, fields: [
            .init(id: "to_host", label: "This API host's registry target (the role is taken where the command runs)", option: "--to-host", required: true),
        ], fixedArguments: ["--authority"]),
        .init(id: "placement-standby", title: "Prepare a registered host to stand by for a placement profile", path: ["placement", "standby"], hostPlacement: .option("--host"), fields: [
            .init(id: "profile", label: "Placement profile", required: true),
            .init(id: "reason", label: "Why this host must stand by (carried in the receipt; each unit is staged, not run)", option: "--reason", required: true),
        ]),
        .init(id: "placement-standby-check", title: "Check whether a registered host could stand by for a placement profile, and what would be delivered (nothing is delivered or written)", path: ["placement", "standby"], hostPlacement: .option("--host"), fields: [
            .init(id: "profile", label: "Placement profile", required: true),
            .init(id: "reason", label: "Why this host would stand by", option: "--reason", required: true),
        ], fixedArguments: ["--check"], mutates: false),
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
        .init(id: "placement-profile-sync-state", title: "Add to a placement profile every store the catalog declares for its products that it does not carry", path: ["placement", "profile", "sync"], hostPlacement: .none, fields: [
            .init(id: "name", label: "Placement profile", required: true),
        ]),
        .init(id: "placement-profile-sync-state-all", title: "Add to every placement profile the stores the catalog declares for its products that it does not carry", path: ["placement", "profile", "sync"], hostPlacement: .none, fixedArguments: ["--all"]),
        .init(id: "placement-profile-draft", title: "Draft the placement profile that would move fixed routes active on one host (nothing is written)", path: ["placement", "profile", "draft"], hostPlacement: .none, fields: [
            .init(id: "name", label: "Profile name", required: true),
            .init(id: "services", label: "Fixed directory routes, one per line, in start order", option: "--service", required: true, multiple: true),
            .init(id: "probes", label: "SERVICE=URL health probes, one per line, for services whose product the catalog declares no GET answering request for", option: "--probe", multiple: true),
            .init(id: "state", label: "Files under the host's home that move, one per line", option: "--state", multiple: true),
            .init(id: "tree", label: "Directories under the host's home that move whole, one per line", option: "--tree-state", multiple: true),
        ], mutates: false, jsonOutput: false),
        .init(id: "placement-transactions", title: "List the placement moves the registry holds open and how long each has been open", path: ["placement", "transactions"], hostPlacement: .none, mutates: false),
        .init(id: "placement-release", title: "Release a placement move whose process ended: roll back its steps and free its services", path: ["placement", "release"], hostPlacement: .none, fields: [
            .init(id: "id", label: "Placement transaction id (as Placement transactions lists it)", required: true),
        ]),
    ]
}
