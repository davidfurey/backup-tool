pub mod encryption;
pub mod decryption;
pub mod sharding;
pub mod swift;
pub mod datastore;
pub mod metadata_file;
pub mod sqlite_cache;
pub mod hash;
pub mod filetype;
pub mod config;
pub mod upload_worker;
pub mod hash_worker;
pub mod backup;

use config::BackupConfig;

extern crate serde;
#[macro_use]
extern crate serde_derive;

fn main() {
    let content = std::fs::read_to_string("backup.toml").unwrap();
    let config: BackupConfig = toml::from_str(&content).unwrap();

    backup::run_backup(config);
}
