//! Adoption records: which pre-existing resource this control plane took
//! responsibility for, keyed by a digest of the resource id.

use sha2::{Digest, Sha256};

use crate::autonomy::model::AdoptionRecord;
use crate::autonomy::storage::objects::{load_records, write_json};
use crate::autonomy::storage::ADOPTION_PREFIX;
use crate::queue::{JobStorage, StorageError};

pub async fn write_adoption(
    store: &JobStorage,
    adoption: &AdoptionRecord,
) -> Result<(), StorageError> {
    let key = hex::encode(Sha256::digest(adoption.resource_id.as_bytes()));
    let path = format!("{ADOPTION_PREFIX}/{key}.json");
    write_json(store, &path, adoption, true).await
}

pub async fn list_adoptions(store: &JobStorage) -> Result<Vec<AdoptionRecord>, StorageError> {
    load_records(store, &format!("{ADOPTION_PREFIX}/")).await
}
