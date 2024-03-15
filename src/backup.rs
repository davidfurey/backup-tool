use std::fs::File;

use indicatif::{MultiProgress, ProgressBar, ProgressFinish, ProgressStyle};
use sequoia_openpgp::Cert;
use sequoia_openpgp::parse::Parse;
use walkdir::WalkDir;

use crate::datastore::DataStore;
use crate::sqlite_cache::AsyncCache;
use crate::swift::Bucket;
use crate::{config, upload_worker, hash_worker, encryption};
use config::BackupConfig;
use chrono::prelude::{Utc, SecondsFormat};
use rand::{distributions::Alphanumeric, Rng};

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

  use futures::StreamExt;
  let stream: futures::stream::Iter<walkdir::IntoIter> = futures::stream::iter(WalkDir::new(&config.source));
  let random_suffix: String = rand::thread_rng()
    .sample_iter(&Alphanumeric)
    .take(4)
    .map(char::from)
    .collect();
  let datetime = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
  let metadata_filename = format!("backup-{}-{}.metadata.sqlite", datetime, random_suffix);
  let metadata_filename_encrypted = format!("backup-{}-{}.metadata", datetime, random_suffix);
  let metadata_file = config.metadata_cache.clone().join(metadata_filename.clone());
  let metadata_file_encrypted = config.metadata_cache.clone().join(metadata_filename_encrypted.clone());
  let metadata_writer = crate::metadata_file::MetadataWriter::new(metadata_file.clone()).await;
  let multi_progress = MultiProgress::new();

  let config = &config;
  let cache = &cache;
  let multi_progress = &multi_progress;
  let buckets = &buckets;
  let v = stream
    .enumerate()
    .map(|(index, dir_entry)| async move {
    let result = match dir_entry {
      Ok(entry) => {
        hash_worker::hash_work(entry, index, &cache, &config.stores.to_vec(), &config.hmac_secret, &multi_progress).await
      },
      Err(_) => { (None, None) }
    };
    match result.0 {
      Some(upload_request) => {
        let requires_upload = cache.requires_upload(&upload_request.data_hash, &config.stores.to_vec()).await.unwrap();
        if !requires_upload.is_empty() && cache.lock_data(&upload_request.data_hash).await { // check here if it is in the database?
          // check here if it is encrypted on the filesystem?
          let key = Cert::from_file(&config.encrypting_key_file).unwrap();
          let upload_request2 = upload_worker::encryption_work(&config.data_cache, upload_request, &key, multi_progress).await;
          let filtered_buckets: Vec<&(DataStore, Bucket, Bucket)> = requires_upload.iter().flat_map(|bucket_id| {
            buckets.iter().find(|bucket| bucket.0.id == *bucket_id)
          }).collect();
          let report = upload_worker::upload(upload_request2, &filtered_buckets, multi_progress).await;
          cache.set_data_in_cold_storage(&report.data_hash.as_str(), "md5_hash", &report.store_ids).await.unwrap();
          std::fs::remove_file(report.filename).unwrap();
        }
      },
      _ => {
      }
    }
    result.1
  });
  v.buffered(64).for_each(|x| async { // does for_each here mean that the buffered(64) is moot?
    match x {
      Some(metadata) => {
        metadata_writer.write(&metadata).await.unwrap();
      },
      None => {}
    }
  }).await;
  metadata_writer.close().await;
  let mut source = File::open(&metadata_file).unwrap();
  let mut dest = File::create(&metadata_file_encrypted).unwrap();
  let encrypting_key = Cert::from_file(config.encrypting_key_file.clone()).unwrap();
  encryption::encrypt_file(&mut source, &mut dest, &encrypting_key).unwrap();
  
  for store in config.stores.iter() {
    let x = store.metadata_bucket().await;
    let metadata_file = std::fs::File::open(&metadata_file_encrypted).unwrap();

    let pb = multi_progress.add(ProgressBar::new(metadata_file.metadata().unwrap().len()))
      .with_finish(ProgressFinish::AndLeave);
    let style =
          ProgressStyle::with_template("{prefix:.bold.dim} {spinner:.green} [{elapsed_precise}] {msg} [{wide_bar:.cyan/blue}] {bytes}/{total_bytes}")
              .unwrap()
              .progress_chars("#>-");

    pb.set_style(style.clone());
    pb.set_message(format!("{}", &metadata_filename_encrypted));
    pb.set_prefix("[Upload] ");
    let callback = move |bytes: usize| {
        pb.inc(u64::try_from(bytes).unwrap_or(0));
        if pb.is_finished() {
            //pb.finish_and_clear();
        }
    };

    x.upload_with_progress(&metadata_filename_encrypted, metadata_file, callback).await.unwrap();
  }
  std::fs::remove_file(&metadata_file).unwrap();
  std::fs::remove_file(&metadata_file_encrypted).unwrap();

  cache.cleanup().await;
  cache.close().await;
  return ()

}