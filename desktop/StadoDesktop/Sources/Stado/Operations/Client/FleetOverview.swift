import Foundation

/// The part of `stado overview --json` the Hosts, Queue and Posture screens
/// read. Every field is required: a document without one is refused naming
/// it, never filled with a zero or an empty list that reads as an idle fleet.
struct FleetOverview: Decodable, Sendable {
    let generatedAt: String
    let jobs: Jobs
    let outcomes: Outcomes
    let fleet: Fleet

    enum CodingKeys: String, CodingKey {
        case generatedAt = "generated_at"
        case jobs, outcomes, fleet
    }

    struct Jobs: Decodable, Sendable {
        let queue: Int
        let running: Int
        let completed: Int
        let failed: Int
    }

    struct Outcomes: Decodable, Sendable {
        let byModel: [ModelCounts]
        let failed: [Failed]
        let completed: [Completed]
        let throughput: Measured

        enum CodingKeys: String, CodingKey {
            case byModel = "by_model"
            case failed, completed, throughput
        }

        struct ModelCounts: Decodable, Sendable {
            /// Nil when the job commands name no model.
            let model: String?
            let queue: Int
            let running: Int
            let completed: Int
            let failed: Int
        }

        struct Failed: Decodable, Sendable {
            let jobID: String
            let model: String?
            let task: String?
            let error: String?

            enum CodingKeys: String, CodingKey {
                case jobID = "job_id"
                case model, task, error
            }
        }

        struct Completed: Decodable, Sendable {
            let jobID: String
            let model: String?
            let task: String?
            let wallSeconds: Int?
            let completedAt: String?

            enum CodingKeys: String, CodingKey {
                case jobID = "job_id"
                case model, task
                case wallSeconds = "wall_seconds"
                case completedAt = "completed_at"
            }
        }

        struct Measured: Decodable, Sendable {
            let averageWallSecondsPerCompletedJob: Double?
            let samples: Int

            enum CodingKeys: String, CodingKey {
                case averageWallSecondsPerCompletedJob = "average_wall_seconds_per_completed_job"
                case samples
            }
        }
    }

    struct Fleet: Decodable, Sendable {
        let workers: [Worker]
        let targets: [Target]

        /// One live capacity publisher, as the overview lists it.
        struct Worker: Decodable, Sendable {
            let consumerID: String
            let target: String?
            let kind: String?
            let publishedAt: String?
            let acceptingJobs: Bool?
            let runningJobs: Int?
            let availableCPUCores: Int?
            let totalCPUCores: Int?
            let availableAccelerators: [String: Int]
            let freeRAMGB: Double?
            let totalRAMGB: Double?
            let freeVRAMGB: Double?
            let totalVRAMGB: Double?

            enum CodingKeys: String, CodingKey {
                case consumerID = "consumer_id"
                case target, kind
                case publishedAt = "published_at"
                case acceptingJobs = "accepting_jobs"
                case runningJobs = "running_jobs"
                case availableCPUCores = "available_cpu_cores"
                case totalCPUCores = "total_cpu_cores"
                case availableAccelerators = "available_accelerators"
                case freeRAMGB = "free_ram_gb"
                case totalRAMGB = "total_ram_gb"
                case freeVRAMGB = "free_vram_gb"
                case totalVRAMGB = "total_vram_gb"
            }
        }

        /// One registry target. `activeWorker` is nil for a target that is not
        /// a local worker, false for a local worker that is not publishing.
        struct Target: Decodable, Sendable {
            let name: String
            let kind: String?
            let activeWorker: Bool?
            let gpuType: String?

            enum CodingKeys: String, CodingKey {
                case name, kind
                case activeWorker = "active_worker"
                case gpuType = "gpu_type"
            }
        }
    }

    /// The command the fleet screens run.
    static let arguments = ["overview", "--json"]
}

extension DashboardSnapshot {
    /// The screens' view of one overview document. A model the job commands
    /// do not name is listed under that sentence rather than a made-up name.
    init(overview: FleetOverview) {
        let generated = StadoFormat.date(overview.generatedAt)
        let gpuByTarget = Dictionary(
            overview.fleet.targets.compactMap { target in target.gpuType.map { (target.name, $0) } },
            uniquingKeysWith: { first, _ in first }
        )
        let live = overview.fleet.workers.map { worker in
            WorkerNode(
                targetName: worker.target,
                consumerID: worker.consumerID,
                declared: worker.target != nil,
                status: .live,
                availabilityReason: "Publishing capacity inside the staleness horizon.",
                kind: worker.kind,
                hostnames: [],
                gpuType: worker.target.flatMap { gpuByTarget[$0] },
                role: nil,
                acceptingJobs: worker.acceptingJobs,
                runningJobs: worker.runningJobs,
                availableCPUCores: worker.availableCPUCores,
                totalCPUCores: worker.totalCPUCores,
                availableAccelerators: worker.availableAccelerators,
                freeRAMGB: worker.freeRAMGB,
                totalRAMGB: worker.totalRAMGB,
                freeVRAMGB: worker.freeVRAMGB,
                totalVRAMGB: worker.totalVRAMGB,
                publishedAt: worker.publishedAt,
                ageSeconds: Self.age(of: worker.publishedAt, at: generated)
            )
        }
        let silent = overview.fleet.targets
            .filter { $0.activeWorker == false }
            .map { target in
                WorkerNode(
                    targetName: target.name,
                    consumerID: nil,
                    declared: true,
                    status: .unavailable,
                    availabilityReason:
                        "A declared local worker with no capacity report inside the staleness horizon.",
                    kind: target.kind,
                    hostnames: [],
                    gpuType: target.gpuType,
                    role: nil,
                    acceptingJobs: nil,
                    runningJobs: nil,
                    availableCPUCores: nil,
                    totalCPUCores: nil,
                    availableAccelerators: [:],
                    freeRAMGB: nil,
                    totalRAMGB: nil,
                    freeVRAMGB: nil,
                    totalVRAMGB: nil,
                    publishedAt: nil,
                    ageSeconds: nil
                )
            }
        var byModel: [String: JobCounts] = [:]
        for row in overview.outcomes.byModel {
            let name: String
            if let model = row.model {
                name = model
            } else {
                name = "Commands that name no model"
            }
            byModel[name] = JobCounts(
                queue: row.queue, running: row.running, completed: row.completed, failed: row.failed
            )
        }
        self.init(
            generatedAt: overview.generatedAt,
            counts: JobCounts(
                queue: overview.jobs.queue,
                running: overview.jobs.running,
                completed: overview.jobs.completed,
                failed: overview.jobs.failed
            ),
            byModelState: byModel,
            liveAgents: live,
            workers: live + silent,
            recentFailed: overview.outcomes.failed.map {
                FailedJob(jobID: $0.jobID, model: $0.model, task: $0.task, error: $0.error)
            },
            completedRecent: overview.outcomes.completed.map {
                CompletedJob(
                    jobID: $0.jobID,
                    model: $0.model,
                    task: $0.task,
                    wallSeconds: $0.wallSeconds.map(Double.init),
                    completedAt: $0.completedAt
                )
            },
            throughput: Throughput(
                averageWallSecondsPerCompletedJob:
                    overview.outcomes.throughput.averageWallSecondsPerCompletedJob,
                samples: overview.outcomes.throughput.samples
            )
        )
    }

    /// Seconds between a capacity report and the document, when both read.
    private static func age(of published: String?, at generated: Date?) -> Double? {
        guard let generated, let published = StadoFormat.date(published) else { return nil }
        return generated.timeIntervalSince(published)
    }
}
