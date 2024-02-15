use crate::datastore;

use std::path::PathBuf;
use datastore::DataStore;

#[derive(Deserialize)]
pub struct BackupConfig {
    pub source: PathBuf,
    pub data_cache: PathBuf,
    pub metadata_cache: PathBuf,
    pub stores: Vec<DataStore>,
    pub hmac_secret: String,
    pub encrypting_key_file: PathBuf,
    pub signing_key_file: Option<PathBuf>,
}