use std::path::PathBuf;
use std::fs;
use std::fs::File;
use std::thread;
use std::fs::remove_file;

use sequoia_openpgp::Cert;

use crate::{datastore, swift, sqlite_cache, encryption};
use datastore::DataStore;
use swift::Bucket;
use sqlite_cache::Cache;
use sqlite_cache::AsyncCache;
use tokio_stream::wrappers::ReceiverStream;
use futures::StreamExt;

#[derive(Debug)]
pub struct UploadRequest {
    pub filename: std::path::PathBuf,
    pub data_hash: String,
}

pub struct UploadReport {
    pub filename: std::path::PathBuf,
    pub data_hash: String,
    pub store_ids: Vec<i32>,
}

pub fn create_encryption_workers(
    stores: Vec<DataStore>,
    data_cache: &PathBuf,
    key: &Cert,
    encryption_rx: Vec<std::sync::mpsc::Receiver<UploadRequest>>,
    upload_tx: tokio::sync::mpsc::Sender<UploadRequest>
) {
  for r in encryption_rx {
    create_encryption_worker(r, stores.clone(), upload_tx.clone(), &data_cache, &key);
  }
}

pub async fn create_uploader(
    upload_rx: tokio::sync::mpsc::Receiver<UploadRequest>,
    buckets: Vec<(DataStore, Bucket, Bucket)>
) {
    let cache = AsyncCache::new().await;
    let c = ReceiverStream::new(upload_rx).map(|request| async {
        let report = upload(request, &buckets).await;
        cache.set_data_in_cold_storage(&report.data_hash.as_str(), "md5_hash", &report.store_ids).await.unwrap();
        remove_file(report.filename).unwrap();
    }).buffer_unordered(64).count().await;
    println!("HERE:Uploaded {:?} files", c);
}

async fn upload(request: UploadRequest, buckets: &Vec<(DataStore, Bucket, Bucket)>) -> UploadReport {
    print!("Uploading {:?}\n", request.data_hash);
    let mut success_ids: Vec<i32> = Vec::new();
    for (store, bucket, _) in buckets.iter() {
        let encrypted_file = fs::File::open(&request.filename).unwrap();
        let key = format!("{}{}", store.data_prefix, request.data_hash);
        match bucket.upload(&key, encrypted_file).await {
            Ok(_) => {
                success_ids.push(store.id);
            },
            _ => {
                print!("Failed to upload {:?} to {:?}\n", request.data_hash, store.id)
            }
        }
    }
    UploadReport { filename: request.filename, data_hash: request.data_hash, store_ids: success_ids }
}

fn create_encryption_worker(upload_rx: std::sync::mpsc::Receiver<UploadRequest>, stores: Vec<DataStore>, uploader: tokio::sync::mpsc::Sender<UploadRequest>, data_cache: &PathBuf, key: &Cert) {
  let cache = Cache::new();

  let data_cache = data_cache.clone();
  let key = key.clone();

  thread::spawn(move || {
    while let Ok(request) = upload_rx.recv() {
        let destination_filename = data_cache.join(&request.data_hash);
        if destination_filename.exists() || cache.is_data_in_cold_storage(&request.data_hash, &stores).unwrap() {
            print!("Skipping {:?} ({:?} already uploaded or in progress)\n", &request.filename, request.data_hash);
        } else {
            print!("Processing {:?}\n", &request.filename);
            {
                let mut source = fs::File::open(request.filename).unwrap();
                print!("Creating {:?}\n", destination_filename);
                let mut dest = File::create(&destination_filename).unwrap();
                encryption::encrypt_file(&mut source, &mut dest, &key).unwrap();
            }
            uploader.blocking_send(UploadRequest { filename: destination_filename, data_hash: request.data_hash }).unwrap();
        }
    }
  });
}