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

use std::sync::Arc;
use std::thread;
use std::sync::mpsc::channel;

use sequoia_openpgp::Cert;
use sequoia_openpgp::parse::Parse;
use walkdir::WalkDir;

use datastore::DataStore;
use config::BackupConfig;
use sqlite_cache::Cache;
use filetype::FileType;
use upload_worker::create_upload_workers;
use hash_worker::create_hash_workers;

extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate rmp_serde as rmps;

fn run_backup(config: BackupConfig) {
    Cache::init();

    let (metadata_tx, metadata_rx) = channel();
    let (hash_tx, hash_rx) = crossbeam_channel::unbounded();

    thread::spawn(move|| {
        for entry in WalkDir::new(config.source) {
            hash_tx.send(entry.unwrap()).unwrap();
        }
    });

    let stores = Arc::new(config.stores);


    let sender = {
        let key = Cert::from_file(config.key_file).unwrap();
        let (upload_channel, s) = create_upload_workers(stores.clone(), &config.data_cache, &key);

        create_hash_workers(hash_rx, metadata_tx, &upload_channel, &stores, config.hmac_secret);
        s
    };

    metadata_file::write_metadata_file(&config.metadata_cache, metadata_rx, stores.clone());

    if sender.recv().is_ok() {
        println!("Unexpected result")
    }

    Cache::cleanup();
}


fn main() {
    let content = std::fs::read_to_string("backup.toml").unwrap();
    let config: BackupConfig = toml::from_str(&content).unwrap();

    run_backup(config);
}
