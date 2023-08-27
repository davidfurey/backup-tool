use std::path::PathBuf;
use std::fs;
use std::fs::File;
use std::thread;

use sequoia_openpgp::Cert;

use crate::{datastore, swift, sqlite_cache, encryption};
use datastore::DataStore;
use swift::Bucket;
use sqlite_cache::Cache;
use tokio_stream::wrappers::ReceiverStream;
use futures::StreamExt;

#[derive(Debug)]
pub struct UploadRequest {
    pub filename: std::path::PathBuf,
    pub data_hash: String,
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
    buckets: Vec<(DataStore, Bucket, Bucket)>, 
    data_cache: &PathBuf, 
    key: &Cert, 
) {
    let data_cache = data_cache.clone();
    let key = key.clone();
    let data_cache = data_cache.clone();   
    let c = ReceiverStream::new(upload_rx).map(|request| async {
        upload_multiple(request, &data_cache, &buckets, &key).await
    }).buffer_unordered(64).count().await;
    println!("Done with uploads (uploader): {:?}", c);
}

async fn upload_multiple(request: UploadRequest, data_cache: &PathBuf, buckets: &Vec<(DataStore, Bucket, Bucket)>, key: &Cert) {
    print!("Uploading {:?} ({:?})\n", request.filename, request.data_hash);
    // let destination_filename = data_cache.join(&request.data_hash);
    // {
    //     let mut source = fs::File::open(request.filename).unwrap();
    //     print!("Creating {:?}\n", destination_filename);
    //     let mut dest = File::create(&destination_filename).unwrap();
    //     encryption::encrypt_file(&mut source, &mut dest, &key).unwrap();
    // }

    for (store, bucket, _) in buckets.iter() {
        let encrypted_file = fs::File::open(&request.filename).unwrap();
        let key = format!("{}{}", store.data_prefix, request.data_hash);
        match bucket.upload(&key, encrypted_file).await {
            Ok(_) => {
                //Ok(store.id);
            },
            _ => {
                //Err(store.id);
            }
        }
    }
    println!("Uploaded");
}

fn create_encryption_worker(upload_rx: std::sync::mpsc::Receiver<UploadRequest>, stores: Vec<DataStore>, uploader: tokio::sync::mpsc::Sender<UploadRequest>, data_cache: &PathBuf, key: &Cert) {
  let cache = Cache::new();

  let data_cache = data_cache.clone();
  let key = key.clone();
  thread::spawn(move || {
    while let Ok(request) = upload_rx.recv() {
        if cache.is_data_in_cold_storage(&request.data_hash, &stores).unwrap() {
            print!("Skipping {:?} ({:?} already uploaded)\n", &request.filename, request.data_hash);
        } else {
            print!("Processing {:?}\n", &request.filename);
            let destination_filename = data_cache.join(&request.data_hash);
            {
                let mut source = fs::File::open(request.filename).unwrap();
                print!("Creating {:?}\n", destination_filename);
                let mut dest = File::create(&destination_filename).unwrap();
                encryption::encrypt_file(&mut source, &mut dest, &key).unwrap();
            }

            uploader.blocking_send(UploadRequest { filename: destination_filename, data_hash: request.data_hash }).unwrap()
            // todo, set in_cold_storage if upload is successful
        }
    }
    print!("Done with uploads\n");
  });
}