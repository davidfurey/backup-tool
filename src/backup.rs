use std::fs::{create_dir_all, File};
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
use crate::utils::humanise_bytes;

async fn init_datastores(stores: Vec<DataStore>) -> Vec<(DataStore, Bucket)> {
  use futures::StreamExt;
  let n = stores.len().max(1);
  futures::stream::iter(stores)
    .map(|store| async move {
      let bucket = store.init().await;
      (store, bucket)
    })
    .buffer_unordered(n)
    .collect()
    .await
}

async fn upload_metadata(key: String, filename: &PathBuf, stores: &Vec<DataStore>, multi_progress: &MultiProgress) {
  let style =
    ProgressStyle::with_template("{prefix:.bold.dim} {spinner:.green} [{elapsed_precise}] {msg} [{wide_bar:.cyan/blue}] {bytes}/{total_bytes}")
      .unwrap()
      .progress_chars("#>-");

  for store in stores.iter() {
    if !store.upload_metadata {
      info!("Skipping metadata upload to store {} (upload_metadata = false)", store.id);
      continue;
    }
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

    let combined_key = format!("{}{}", store.metadata_prefix, key);

    store
      .init()
      .await
      .upload_with_progress(&combined_key, metadata_file, callback)
      .await
      .unwrap();
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

#[derive(Default)]
struct Stats {
  pub files: u64,
  pub unchanged_files: u64,
  pub links: u64,
  pub directories: u64,
  pub uploaded: u64,
  pub size: u64
}

pub async fn run_backup(config: BackupConfig, name: String, multi_progress: MultiProgress, force_hash: bool, dry_run: bool) {

  let cache = AsyncCache::new().await;
  cache.init().await;
  let buckets = init_datastores(config.stores.to_vec()).await;

  create_dir_all(config.metadata_cache.as_path()).unwrap();
  create_dir_all(config.data_cache.as_path()).unwrap();

  let metadata_file = {
    let metadata_filename = format!("{}.metadata.sqlite", name);
    config.metadata_cache.clone().join(metadata_filename.clone())
  };
  
  let metadata_writer = crate::metadata_file::MetadataWriter::new(metadata_file.clone()).await;

  let config = &config;
  let cache = &cache;
  let multi_progress = &multi_progress;
  let buckets = &buckets;

  use futures::StreamExt;
  let directory_stream: futures::stream::Iter<walkdir::IntoIter> = futures::stream::iter(WalkDir::new(&config.source));

  let stats = directory_stream
    .enumerate()
    .map(|(index, dir_entry)| async move {
    let result = match dir_entry {
      Ok(entry) => {
        hash_worker::hash_work(entry, index, &config.source, &cache, &config.stores, &config.hmac_secret, &multi_progress, force_hash).await
      },
      Err(_) => { (None, None, false, 0) }
    };
    let uploaded = match result.0 {
      Some(upload_request) => {
        let x = upload_request.filename.clone();
        let filename = x.to_string_lossy();
        let requires_upload = cache.requires_upload(&upload_request.data_hash, &config.stores).await.unwrap();
        if !requires_upload.is_empty() && cache.lock_data(&upload_request.data_hash).await { // check here if it is in the database?
          // check here if it is encrypted on the filesystem?
          let key = Cert::from_file(&config.encrypting_key_file).unwrap();
          let upload_request2 = upload_worker::encryption_work(&config.data_cache, upload_request, &key, multi_progress).await;
          if !dry_run {
            let filtered_buckets: Vec<&(DataStore, Bucket)> = requires_upload.iter().flat_map(|bucket_id| {
              buckets.iter().find(|b| b.0.id == *bucket_id && b.0.upload_data)
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
  }).buffered(64).fold((Stats::default(), metadata_writer), |(cur, metadata_writer), file_metadata| async move {
    match file_metadata {
      (Some(metadata), hash_cached, size, uploaded) => {
        metadata_writer.write(&metadata).await.unwrap();
        let new_stats = match metadata.ttype {
          filetype::FileType::FILE => Stats {
            files: cur.files + 1,
            unchanged_files: cur.unchanged_files + if hash_cached { 1 } else { 0 },
            uploaded: cur.uploaded + if uploaded { 1 } else { 0 },
            size: cur.size + size,
            ..cur
          },
          filetype::FileType::SYMLINK => Stats {
            links: cur.links + 1,
            ..cur
          },
          filetype::FileType::DIRECTORY => Stats {
            directories: cur.directories + 1,
            ..cur
          }
        };
        (new_stats, metadata_writer)
      },
      _ => {
        (cur, metadata_writer)
      }
    }
  }).await;
  let (stats, metadata_writer) = stats;

  metadata_writer.write_metadata("size", stats.size.to_string().as_str()).await;
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