use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::fs;
use std::fs::File;
use std::sync::mpsc::channel;

use sequoia_openpgp::Cert;

use crate::{datastore, sharding, swift, sqlite_cache, encryption};
use datastore::DataStore;
use sharding::ShardedChannel;
use swift::Bucket;
use sqlite_cache::Cache;

#[derive(Debug)]
pub struct UploadRequest {
    pub filename: std::path::PathBuf,
    pub data_hash: String,
}

pub fn create_upload_workers(stores: Arc<Vec<DataStore>>, data_cache: &PathBuf, key: &Cert) -> (ShardedChannel<UploadRequest>, std::sync::mpsc::Receiver<()>) {
  let (tx, rx) = channel::<()>();
  (sharding::ShardedChannel::new(4, |f| {
      create_upload_worker(f, stores.clone(), data_cache, key, tx.clone())
  }), rx)
}

fn create_upload_worker(upload_rx: std::sync::mpsc::Receiver<UploadRequest>, stores: Arc<Vec<DataStore>>, data_cache: &PathBuf, key: &Cert, join: std::sync::mpsc::Sender<()>) {
//    static mut UPLOAD_ID_COUNT: u32 = 1;
  // let id = unsafe {
  //     UPLOAD_ID_COUNT += 1;
  //     UPLOAD_ID_COUNT
  // };
  let key = key.clone();
  let data_cache = data_cache.clone();
  thread::spawn(move|| {
      let _x = join;
      let buckets: Vec<(&DataStore, Bucket)> = stores.iter().map(|store| {
          (store, store.init())
      }).collect();
      let cache = Cache::new();

      while let Ok(request) = upload_rx.recv() {
          if cache.is_data_in_cold_storage(&request.data_hash, &stores).unwrap() {
              print!("Skipping {:?} ({:?} already uploaded)\n", &request.filename, request.data_hash);
          } else {
              print!("Uploading {:?} ({:?})\n", request.filename, request.data_hash);
              let destination_filename = data_cache.join(&request.data_hash);
              {
                  let mut source = fs::File::open(request.filename).unwrap();
                  let mut dest = File::create(&destination_filename).unwrap();
                  encryption::encrypt_file(&mut source, &mut dest, &key).unwrap();
              }

              for (store, bucket) in buckets.as_slice() {
                  let encrypted_file = fs::File::open(&destination_filename).unwrap();
                  let key = format!("{}{}", store.data_prefix, request.data_hash);
                  match bucket.upload(&key, encrypted_file) {
                      Ok(_) => {
                          cache.set_data_in_cold_storage(request.data_hash.as_str(), "", &stores).unwrap();
                      },
                      _ => {
                          // throw exception here?
                      }
                  }
              }
          }
      }
      print!("Done with uploads\n");
  });
}