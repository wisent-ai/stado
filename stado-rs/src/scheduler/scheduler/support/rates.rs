//! Accelerator pricing for the local-pack savings score, read from the live
//! quotes the coordinator keeps at [`PRICE_BOOK_PATH`]. No rate is written in
//! code: an accelerator no quote names has no rate.

use crate::autonomy::cost::PriceBook;
use crate::queue::{JobStorage, StorageError};

/// Where the coordinator keeps the provider quotes it last read.
pub const PRICE_BOOK_PATH: &str = "state/autonomy/cost/prices.json";

/// The stored price book, or `None` when the coordinator has written none.
pub async fn stored_price_book(store: &JobStorage) -> Result<Option<PriceBook>, StorageError> {
    crate::autonomy::storage::read_json::<PriceBook>(store, PRICE_BOOK_PATH).await
}

/// The cheapest quoted hourly price of one accelerator, for the purchase
/// `preemptible` names; `None` when no quote names it.
pub fn accel_hourly_rate(book: &PriceBook, accel_type: &str, preemptible: bool) -> Option<f64> {
    book.cheapest_accelerator_hourly(accel_type, preemptible)
}
