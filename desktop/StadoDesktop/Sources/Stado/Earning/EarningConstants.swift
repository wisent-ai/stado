import Foundation

/// The values the earning form opens on. `stado market` has no price or
/// window of its own: every one is an argument, and these are the starting
/// values of the fields that supply them.
enum EarningConstants {
    /// Starting `--price-gpu`, in US dollars per GPU-hour.
    static let defaultPriceGPU: Double = 0.50
    /// Starting `--price-disk`, in US dollars per GB-month.
    static let defaultPriceDisk: Double = 0.05
    /// Starting `--idle-window-s` for the preview.
    static let defaultIdleWindowSeconds: Int = 300
    /// `--max-duration-s` the preview evaluates with: the longest single
    /// rental an offer allows.
    static let maxRentalSeconds: Int = 3600
    /// The idle windows the preview stepper offers: from listing the moment
    /// the queue empties up to an hour, which is also the default cap on a
    /// single rental.
    static let idleWindowRange: ClosedRange<Int> = 0...3600
    /// One minute per press, so the stepper crosses the five-minute default
    /// in five presses instead of three hundred.
    static let idleWindowStepSeconds: Int = 60
    /// Points of width for the price field: a dollar amount with cents.
    static let priceFieldWidth: CGFloat = 120
    /// The idle window that decides on the first poll: the bridge lists as
    /// soon as the queue is empty, which is what a preview wants to show.
    static let immediateIdleWindowSeconds: Int = 0
}
