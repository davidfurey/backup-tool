use std::thread;
use std::sync::mpsc::channel;

//use futures::StreamExt;
use indicatif::MultiProgress;
use sequoia_openpgp::Cert;
use sequoia_openpgp::parse::Parse;
use walkdir::WalkDir;

use crate::datastore::DataStore;
use crate::sqlite_cache::AsyncCache;
use crate::swift::Bucket;
use crate::{config, upload_worker, hash_worker, metadata_file, sharding};
use config::BackupConfig;
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

  let cache = AsyncCache::new().await;
  cache.init().await;

  let m = MultiProgress::new();
// Directory listing (thread) -> hash channel -> hash worker {-> encryption (sharded) channel -> uploader stream
//                                                           {-> metadata channel
  let (metadata_tx, metadata_rx) = channel();
  //let (hash_tx, hash_rx) = crossbeam_channel::bounded(16);

  let buckets = init_datastores(config.stores.to_vec()).await;

  let (encryption_rx, encryption_channel) = 
    sharding::ShardedChannel::new_vec(4);
  let (upload_tx, upload_channel) = tokio::sync::mpsc::channel(16); // todo: optimum buffer size?

  let hmac_secret = config.hmac_secret.clone().to_owned();
  let source = config.source.clone();
  let stores = config.stores.to_vec();
  let cloned_cache = cache.clone();
  //thread::spawn(move || {
  tokio::task::spawn(async move {
    // use rayon::prelude::*;
    use tokio_stream::StreamExt;
    let mut stream = tokio_stream::iter(WalkDir::new(source));
    while let Some(dir_entry) = stream.next().await {
      let result = match dir_entry {
        Ok(entry) => hash_worker::hash_work(entry, &cloned_cache, &stores, &hmac_secret).await,
        Err(_) => { (None, None) }
      };
      match result {
        (Some(upload_request), Some(metadata)) => { 
          let dh1 = upload_request.data_hash.clone().to_owned();
          let dh = dh1.as_str();
          encryption_channel.send(upload_request, dh).unwrap();
          metadata_tx.send(metadata).unwrap();
        },
        _ => {}
      }
    }
    //.buffer_unordered(64).count().await;
  });

//   let c = ReceiverStream::new(upload_rx).map(|request| async {
//     pb.inc(1);
//     pb.set_message(format!("{}", request.data_hash));
//     let report = upload(request, &buckets).await;
//     cache.set_data_in_cold_storage(&report.data_hash.as_str(), "md5_hash", &report.store_ids).await.unwrap();
//     remove_file(report.filename).unwrap();
// }).buffer_unordered(64).count().await;

  {
    let key = Cert::from_file(config.key_file.clone()).unwrap();
    let stores = config.stores.to_vec();
    create_encryption_workers(stores, &config.data_cache, &key, encryption_rx, upload_tx, &m);
  }

  // let handles = {
  //   let stores = config.stores.to_vec();
  //   create_hash_workers(hash_rx, metadata_tx, encryption_channel, stores, config.hmac_secret, &m, cache.clone())
  // };
    
  let a = {
    let stores = config.stores.to_vec();
    let key = Cert::from_file(config.key_file.clone()).unwrap();
    let mc = config.metadata_cache.clone();
    tokio::task::spawn(async move {
      metadata_file::write_metadata_file(&mc, metadata_rx, stores, &key).await
    })
  };

  let b = {
    tokio::task::spawn(async move {
      create_uploader(upload_channel, buckets, &m).await
    })
  };

  a.await.unwrap();
  b.await.unwrap();

  cache.cleanup().await;
}