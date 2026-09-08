import Foundation

extension JobCounts {
    static let zero = JobCounts(queue: 0, running: 0, completed: 0, failed: 0)
}

extension Throughput {
    static let unavailable = Throughput(
        averageWallSecondsPerCompletedJob: nil,
        samples: 0,
        projectedRemainingSeconds: nil
    )
}
