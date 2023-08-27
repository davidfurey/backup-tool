use std::thread;
use std::sync::mpsc::channel;

use sequoia_openpgp::Cert;
use sequoia_openpgp::parse::Parse;
use walkdir::WalkDir;

use crate::datastore::DataStore;
use crate::swift::Bucket;
use crate::{config, sqlite_cache, upload_worker, hash_worker, metadata_file, sharding};
use config::BackupConfig;
use sqlite_cache::Cache;
use upload_worker::create_encryption_workers;
use upload_worker::create_uploader;
use hash_worker::create_hash_workers;

pub async fn init_datastore(store: &DataStore) -> (DataStore, Bucket, Bucket) {
  (store.clone(), store.init().await, store.metadata_bucket().await)
}

pub async fn init_datastores(stores: Vec<DataStore>) -> Vec<(DataStore, Bucket, Bucket)> {
  tokio::task::spawn(async move {
    let mut buckets: Vec<(DataStore, Bucket, Bucket)> = Vec::new();
    for bucket in stores.iter() {
      buckets.push(init_datastore(bucket).await)
    }
    buckets
  }).await.unwrap()
}


pub async fn run_backup(config: BackupConfig) {
  Cache::init();

// Directory listing (thread) -> hash channel -> hash worker {-> encryption (sharded) channel -> uploader stream
//                                                           {-> metadata channel
  let (metadata_tx, metadata_rx) = channel();
  let (hash_tx, hash_rx) = crossbeam_channel::bounded(16);

  let buckets = init_datastores(config.stores.to_vec()).await;

  thread::spawn(move || {
      for entry in WalkDir::new(config.source) {
          hash_tx.send(entry.unwrap()).unwrap();
      }
  });

  let (upload_tx, upload_channel) = 
    tokio::sync::mpsc::channel(16); // todo: optimum buffer size?
  let (encryption_rx, encryption_channel) = 
    sharding::ShardedChannel::new_vec(4);

  {
    let key = Cert::from_file(config.key_file.clone()).unwrap();
    let stores = config.stores.to_vec();
    create_encryption_workers(stores, &config.data_cache, &key, encryption_rx, upload_tx);
  }

  {
    let stores = config.stores.to_vec();
    create_hash_workers(hash_rx, metadata_tx, encryption_channel, stores, config.hmac_secret);
  }
    
  let a = {
    let stores = config.stores.to_vec();
    let key = Cert::from_file(config.key_file.clone()).unwrap();
    let mc = config.metadata_cache.clone();
    tokio::task::spawn(async move {
      metadata_file::write_metadata_file(&mc, metadata_rx, stores, &key).await
    })
  };

  let b = {
    let key = Cert::from_file(config.key_file.clone()).unwrap();
    tokio::task::spawn(async move {
      create_uploader(upload_channel, buckets, &config.data_cache, &key).await
    })
  };

  a.await;
  b.await;

  Cache::cleanup();
}