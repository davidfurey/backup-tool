use std::fs::File;
use std::path::PathBuf;

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
use log::info;

use crate::filetype;

async fn init_datastore(store: &DataStore) -> (DataStore, Bucket, Bucket) {
  (store.clone(), store.init().await, store.metadata_bucket().await)
}

async fn init_datastores(stores: Vec<DataStore>) -> Vec<(DataStore, Bucket, Bucket)> {
  tokio::task::spawn(async move {
    let mut buckets: Vec<(DataStore, Bucket, Bucket)> = Vec::new();
    for bucket in stores.iter() {
      buckets.push(init_datastore(bucket).await)
    }
    buckets
  }).await.unwrap()
}

async fn upload_metadata(key: String, filename: &PathBuf, stores: &Vec<DataStore>, multi_progress: &MultiProgress) {
  let style =
    ProgressStyle::with_template("{prefix:.bold.dim} {spinner:.green} [{elapsed_precise}] {msg} [{wide_bar:.cyan/blue}] {bytes}/{total_bytes}")
      .unwrap()
      .progress_chars("#>-");

  for store in stores.iter() {
    // todo: key should be prefixes with store.metadata_prefix, right?
    let metadata_file = std::fs::File::open(&filename).unwrap();

    let pb = multi_progress.add(ProgressBar::new(metadata_file.metadata().unwrap().len()))
      .with_finish(ProgressFinish::AndLeave);
    pb.set_style(style.clone());
    pb.set_message(format!("{}", &key));
    pb.set_prefix("[Upload] ");

    let callback = move |bytes: usize| {
        pb.inc(u64::try_from(bytes).unwrap_or(0));
        if pb.is_finished() {
            //pb.finish_and_clear();
        }
    };

    store
      .metadata_bucket()
      .await
      .upload_with_progress(&key, metadata_file, callback)
      .await
      .unwrap();
  }
}

fn humanise_bytes(b: u64) -> String {
  if b > 1024*1024*1024 {
    format!("{:.2}GiB", (b as f64) / (1024.0*1024.0*1024.0))
  } else if b > 1024*1024 {
    format!("{:.2}MiB", (b as f64) / (1024.0*1024.0))
  } else if b > 1024 {
    format!("{:.2}KiB", (b as f64) / 1024.0)
  } else {
    format!("{} bytes", b)
  }
}
pub fn generate_name() -> String{
  let datetime = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
  let random_suffix: String = rand::thread_rng()
    .sample_iter(&Alphanumeric)
    .take(4)
    .map(char::from)
    .collect();
  format!("backup-{}-{}", datetime, random_suffix)
}

struct Stats {
  pub files: u64,
  pub unchanged_files: u64,
  pub links: u64,
  pub directories: u64,
  pub uploaded: u64,
  pub size: u64
}

pub async fn run_backup(config: BackupConfig, name: String, force_hash: bool, dry_run: bool) {

  let cache = AsyncCache::new().await;
  cache.init().await;
  let buckets = init_datastores(config.stores.to_vec()).await;

  let metadata_file = {
    let metadata_filename = format!("{}.metadata.sqlite", name);
    config.metadata_cache.clone().join(metadata_filename.clone())
  };
  
  let metadata_writer = crate::metadata_file::MetadataWriter::new(metadata_file.clone()).await;

  let multi_progress = MultiProgress::new();

  let config = &config;
  let cache = &cache;
  let multi_progress = &multi_progress;
  let buckets = &buckets;

  use futures::StreamExt;
  let directory_stream: futures::stream::Iter<walkdir::IntoIter> = futures::stream::iter(WalkDir::new(&config.source));

  let empty_stats = Stats {
    files: 0,
    directories: 0,
    unchanged_files: 0,
    links: 0,
    uploaded: 0,
    size: 0
  };
  
  let stats = directory_stream
    .enumerate()
    .map(|(index, dir_entry)| async move {
    let result = match dir_entry {
      Ok(entry) => {
        hash_worker::hash_work(entry, index, &cache, &config.stores.to_vec(), &config.hmac_secret, &multi_progress, force_hash).await
      },
      Err(_) => { (None, None, false, 0) }
    };
    let uploaded = match result.0 {
      Some(upload_request) => {
        let x = upload_request.filename.clone();
        let filename = x.to_string_lossy();
        let requires_upload = cache.requires_upload(&upload_request.data_hash, &config.stores.to_vec()).await.unwrap();
        if !requires_upload.is_empty() && cache.lock_data(&upload_request.data_hash).await { // check here if it is in the database?
          // check here if it is encrypted on the filesystem?
          let key = Cert::from_file(&config.encrypting_key_file).unwrap();
          let upload_request2 = upload_worker::encryption_work(&config.data_cache, upload_request, &key, multi_progress).await;
          if !dry_run {
            let filtered_buckets: Vec<&(DataStore, Bucket, Bucket)> = requires_upload.iter().flat_map(|bucket_id| {
              buckets.iter().find(|bucket| bucket.0.id == *bucket_id)
            }).collect();
            let report = upload_worker::upload(upload_request2, &filtered_buckets, multi_progress).await;
            cache.set_data_in_cold_storage(&report.data_hash.as_str(), "md5_hash", &report.store_ids).await.unwrap();
            std::fs::remove_file(report.filename).unwrap();
          } else {
            info!("Skipping upload of {}", filename);
            std::fs::remove_file(upload_request2.filename).unwrap();
          }
          true
        } else {
          false
        }
      },
      _ => { false
      }
    };
    (result.1, result.2, result.3, uploaded)
  }).buffered(64).fold(empty_stats, |cur, file_metadata| async { // does for_each here mean that the buffered(64) is moot?
    match file_metadata {
      (Some(metadata), hash_cached, size, uploaded) => {
        metadata_writer.write(&metadata).await.unwrap();
        return match metadata.ttype {
          filetype::FileType::FILE => { Stats {
            files: cur.files + 1,
            directories: cur.directories,
            links: cur.links,
            unchanged_files: cur.unchanged_files + if hash_cached { 1 } else { 0 },
            uploaded: cur.uploaded + if uploaded { 1 } else { 0 },
            size: cur.size + size,
          } },
          filetype::FileType::SYMLINK => { Stats {
            files: cur.files,
            directories: cur.directories,
            links: cur.links + 1,
            unchanged_files: cur.unchanged_files,
            uploaded: cur.uploaded,
            size: cur.size
          } },
          filetype::FileType::DIRECTORY => { Stats {
            files: cur.files,
            directories: cur.directories + 1,
            links: cur.links,
            unchanged_files: cur.unchanged_files,
            uploaded: cur.uploaded,
            size: cur.size
          } }
        }
      },
      _ => {
        return cur;
      }
    }
  }).await;

  metadata_writer.close().await;

  let metadata_filename_encrypted = format!("{}.metadata", name);
  let metadata_file_encrypted = config.metadata_cache.clone().join(metadata_filename_encrypted.clone());
  
  {
    let mut source = File::open(&metadata_file).unwrap();
    let mut dest = File::create(&metadata_file_encrypted).unwrap();
    let cert = Cert::from_file(config.encrypting_key_file.clone()).unwrap();
    let signing_key = config.signing_key_file.clone().map(|x| Cert::from_file(x).unwrap());
    encryption::encrypt_file(&mut source, &mut dest, &cert, signing_key).unwrap();
  }
  std::fs::remove_file(&metadata_file).unwrap();
  
  if !dry_run {
    upload_metadata(metadata_filename_encrypted, &metadata_file_encrypted, &config.stores, multi_progress).await;
  } else {
    info!("Skipping upload of metadata")
  }
  std::fs::remove_file(&metadata_file_encrypted).unwrap();

  cache.cleanup().await;
  cache.close().await;

  println!("Processed {} files ({}), {} directories and {} symlinks", stats.files, humanise_bytes(stats.size), stats.directories, stats.links);
  println!("Uploaded: {:}", stats.uploaded);
  println!("Unchanged: {:}", stats.unchanged_files);

  return ()

}