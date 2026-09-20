import Foundation

/// The deadlines and defaults the earning screen runs `stado vast` with.
///
/// They are here rather than at the call sites because each one is a claim
/// about a real measurement: readiness reads a fleet host's vault over its
/// own channel, which took eleven seconds against charless-mac-mini on
/// 2026-09-20 and is bounded well above that; the marketplace calls reach
/// console.vast.ai over the internet. The price and the idle window are the
/// CLI's own defaults, repeated here so the form opens on what the daemon
/// would do unattended.
enum EarningConstants {
    /// Readiness asks the channel, the vault host and Vast.ai in one run.
    static let readinessTimeoutSeconds: Int = 180
    /// A listing call crosses the public internet to console.vast.ai.
    static let marketplaceTimeoutSeconds: Int = 180
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
