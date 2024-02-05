use sequoia_openpgp::Cert;
use sequoia_openpgp::parse::Parse;
use walkdir::WalkDir;

use crate::datastore::DataStore;
use crate::sqlite_cache::AsyncCache;
use crate::swift::Bucket;
use crate::{config, upload_worker, hash_worker};
use config::BackupConfig;

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

  let buckets = init_datastores(config.stores.to_vec()).await;

  let hmac_secret = config.hmac_secret.clone().to_owned();
  let source = config.source.clone();
  let stores = config.stores.to_vec();
  let cloned_cache = cache.clone();

  use futures::StreamExt;
  let stream = futures::stream::iter(WalkDir::new(source));
  let v = stream.map(|dir_entry| async {
    let result = match dir_entry {
      Ok(entry) => hash_worker::hash_work(entry, &cloned_cache, &stores, &hmac_secret).await,
      Err(_) => { (None, None) }
    };
    match result.0 {
      Some(upload_request) => {
        if !cache.is_data_in_cold_storage(&upload_request.data_hash, &stores).await.unwrap() && cache.lock_data(&upload_request.data_hash).await { // check here if it is in the database?
          // check here if it is encrypted on the filesystem?
          let key = Cert::from_file(&config.key_file).unwrap();
          let upload_request2 = upload_worker::encryption_work(&config.data_cache, upload_request, &key).await;
          let report = upload_worker::upload(upload_request2, &buckets).await;
          cache.set_data_in_cold_storage(&report.data_hash.as_str(), "md5_hash", &report.store_ids).await.unwrap();
          std::fs::remove_file(report.filename).unwrap();
        }
      },
      _ => {
      }
    }
    result.1
  });
  let a: Vec<crate::metadata_file::FileMetadata> = v.buffered(64).filter_map(|x| async {
    x
  }).collect().await;

  let key = Cert::from_file(config.key_file.clone()).unwrap();
  let mc = config.metadata_cache.clone();
  crate::metadata_file::write_metadata_file2(&mc, a, stores, &key).await;
  cache.cleanup().await;

  return ()

}