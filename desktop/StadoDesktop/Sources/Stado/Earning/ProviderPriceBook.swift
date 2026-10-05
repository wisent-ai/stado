import Foundation

/// The provider observations stored by Stado, without a second pricing calculation.
struct ProviderPriceBook: Decodable, Sendable {
    struct Quote: Decodable, Sendable {
        let provider: String
        let sku: String
        let description: String
        let region: String?
        let machineType: String?
        let acceleratorType: String?
        let purchaseOption: String
        let unit: String
        let hourlyUsd: Double
        let currency: String
        let source: String
        let observedAt: String
        let dynamic: Bool

        enum CodingKeys: String, CodingKey {
            case provider, sku, description, region, unit, currency, source, dynamic
            case machineType = "machine_type"
            case acceleratorType = "accelerator_type"
            case purchaseOption = "purchase_option"
            case hourlyUsd = "hourly_usd"
            case observedAt = "observed_at"
        }
    }

    struct Source: Decodable, Sendable {
        let provider: String
        let state: String
        let observedAt: String
        let source: String
        let error: String?
        let quotes: [Quote]

        enum CodingKeys: String, CodingKey {
            case provider, state, source, error, quotes
            case observedAt = "observed_at"
        }
    }

    let createdAt: String
    let sources: [Source]
    let quotes: [Quote]

    enum CodingKeys: String, CodingKey {
        case sources, quotes
        case createdAt = "created_at"
    }
}

struct ProviderAllocationQuotes: Decodable, Sendable {
    struct Row: Decodable, Sendable {
        let jobID: String
        let allocation: Allocation?
        let quote: ProviderPriceBook.Quote?
        let error: String?
        enum CodingKeys: String, CodingKey {
            case allocation, quote, error
            case jobID = "job_id"
        }
    }

    struct Allocation: Decodable, Sendable {
        struct Job: Decodable, Sendable {
            let terminal: Bool
            let maxRestarts: Int64?
            let workerAllocation: WorkerAllocation?
            let providerCleanup: ProviderCleanup?
            enum CodingKeys: String, CodingKey {
                case terminal
                case maxRestarts = "max_restarts"
                case providerCleanup = "provider_cleanup"
                case workerAllocation = "worker_allocation"
            }
        }
        let job: Job
    }

    struct WorkerAllocation: Decodable, Sendable {
        enum Resource: Decodable, Sendable {
            case local
            case aws(account: String, region: String, instance: String)
            case gcp(project: String, zone: String, name: String, generation: UInt64)
            case azure(subscription: String, location: String, resource: String, generation: String)

            enum CodingKeys: String, CodingKey {
                case provider, region, zone, name, location
                case accountID = "account_id", instanceID = "instance_id", projectID = "project_id"
                case subscriptionID = "subscription_id", resourceID = "resource_id", vmID = "vm_id"
            }

            init(from decoder: Decoder) throws {
                let fields = try decoder.container(keyedBy: CodingKeys.self)
                switch try fields.decode(String.self, forKey: .provider) {
                case "local": self = .local
                case "aws":
                    self = .aws(account: try fields.decode(String.self, forKey: .accountID),
                        region: try fields.decode(String.self, forKey: .region),
                        instance: try fields.decode(String.self, forKey: .instanceID))
                case "gcp":
                    self = .gcp(project: try fields.decode(String.self, forKey: .projectID),
                        zone: try fields.decode(String.self, forKey: .zone),
                        name: try fields.decode(String.self, forKey: .name),
                        generation: try fields.decode(UInt64.self, forKey: .instanceID))
                case "azure":
                    self = .azure(subscription: try fields.decode(String.self, forKey: .subscriptionID),
                        location: try fields.decode(String.self, forKey: .location),
                        resource: try fields.decode(String.self, forKey: .resourceID),
                        generation: try fields.decode(String.self, forKey: .vmID))
                default:
                    throw DecodingError.dataCorruptedError(forKey: .provider, in: fields,
                        debugDescription: "Unsupported worker identity provider")
                }
            }

            var detail: String {
                switch self {
                case .local: return "Local rate policy; no cloud VM identity is claimed."
                case let .aws(account, region, instance):
                    return "AWS account: \(account) · region: \(region) · instance: \(instance)"
                case let .gcp(project, zone, name, generation):
                    return "GCP project: \(project) · zone: \(zone) · VM: \(name) · generation: \(generation)"
                case let .azure(subscription, location, resource, generation):
                    return "Azure subscription: \(subscription) · location: \(location) · resource: \(resource) · generation: \(generation)"
                }
            }
        }
        let host: String
        let kind: String
        let observedAt: String
        let resource: Resource?
        let error: String?
        enum CodingKeys: String, CodingKey {
            case host, kind, resource, error
            case observedAt = "observed_at"
        }
    }

    struct ProviderCleanup: Decodable, Sendable {
        struct RecordedAllocation: Decodable, Sendable {
            let provider: String
            let instanceRef: String
            let startedAt: String?
            let restarts: Int64
            let source: String
            let capturedAt: String
            let workerAllocation: WorkerAllocation?
            enum CodingKeys: String, CodingKey {
                case provider, restarts, source
                case instanceRef = "instance_ref"
                case startedAt = "started_at"
                case capturedAt = "captured_at"
                case workerAllocation = "worker_allocation"
            }
        }
        let jobID: String
        let operation: String
        let allocation: RecordedAllocation?
        let observedAt: String?
        let removed: Bool?
        let state: String?
        let error: String?
        enum CodingKeys: String, CodingKey {
            case operation, allocation, removed, state, error
            case jobID = "job_id"
            case observedAt = "observed_at"
        }
    }

    struct Source: Decodable, Sendable {
        let provider: String
        let state: String
        let observedAt: String
        let source: String?
        let account: String?
        let error: String?
        let upstreamError: String?
        enum CodingKeys: String, CodingKey {
            case provider, state, source, account, error
            case observedAt = "observed_at"
            case upstreamError = "upstream_error"
        }
    }

    let createdAt: String
    let complete: Bool
    let bookCreatedAt: String?
    let bookError: String?
    let inventorySnapshotID: String?
    let inventoryCreatedAt: String?
    let inventoryError: String?
    let priceSources: [Source]
    let inventorySources: [Source]
    let quotes: [Row]

    enum CodingKeys: String, CodingKey {
        case complete, quotes
        case createdAt = "created_at"
        case bookCreatedAt = "book_created_at"
        case bookError = "book_error"
        case inventorySnapshotID = "inventory_snapshot_id"
        case inventoryCreatedAt = "inventory_created_at"
        case inventoryError = "inventory_error"
        case priceSources = "price_sources"
        case inventorySources = "inventory_sources"
    }
}
