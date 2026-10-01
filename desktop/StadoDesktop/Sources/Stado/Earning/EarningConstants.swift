import Foundation

/// The defaults the earning screen runs `stado vast` with: the CLI's own
/// price and idle window, repeated here so the form opens on what the daemon
/// would do unattended.
enum EarningConstants {
    /// `stado vast auto-list --price-gpu` default, in US dollars per hour.
    static let defaultPriceGPU: Double = 0.50
    /// `stado vast auto-list --idle-window-s` default.
    static let defaultIdleWindowSeconds: Int = 300
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
