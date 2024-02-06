use std::path::PathBuf;
use std::fs;
use std::fs::File;

use log::error;
use log::trace;
use sequoia_openpgp::Cert;

use crate::{datastore, swift, encryption};
use datastore::DataStore;
use swift::Bucket;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

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

pub async fn upload(request: UploadRequest, buckets: &Vec<(DataStore, Bucket, Bucket)>) -> UploadReport {
    trace!("{:?}\n", request.data_hash);
    let mut success_ids: Vec<i32> = Vec::new();
    for (store, bucket, _) in buckets.iter() {
        let encrypted_file = fs::File::open(&request.filename).unwrap();
        let key = format!("{}{}", store.data_prefix, request.data_hash);
        match bucket.upload(&key, encrypted_file).await {
            Ok(_) => {
                success_ids.push(store.id);
            },
            _ => {
                error!("Failed to upload {:?} to {:?}\n", request.data_hash, store.id)
            }
        }
    }
    UploadReport { filename: request.filename, data_hash: request.data_hash, store_ids: success_ids }
}

pub async fn encryption_work(data_cache: &PathBuf, request: UploadRequest, key: &Cert, mp: &MultiProgress) -> UploadRequest {
    let destination_filename = data_cache.join(&request.data_hash);
    //pb.inc(1);
    // if destination_filename.exists() || cache.is_data_in_cold_storage(&request.data_hash, &stores).unwrap() {
    //     pb.set_message(format!("{} [Skipped]", &request.filename.to_string_lossy()));
    //     trace!("Skipping {:?} ({:?} already uploaded or in progress)\n", &request.filename, request.data_hash);
    // } else {
//        pb.set_message(format!("{}", &request.filename.to_string_lossy()));
        trace!("Processing as rayon {:?}\n", &request.filename);
//        {
            let (send, recv) = tokio::sync::oneshot::channel();
            let key = key.clone();
            let spinner_style = ProgressStyle::with_template("{prefix:.bold.dim} {spinner} {wide_msg}")
                .unwrap()
                .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ");
            let mp = mp.clone();

            let filename = format!("{:?}", request.data_hash);
            rayon::spawn(move || {
                let pb = mp.add(ProgressBar::new_spinner());
                pb.set_style(spinner_style.clone());
                pb.set_prefix(format!("[Encrypt]"));
                pb.inc(1);
                pb.set_message(format!("{}", filename));                

                let mut source = fs::File::open(request.filename).unwrap();
                trace!("Creating {:?}\n", destination_filename);
                let mut dest = File::create(&destination_filename).unwrap();
                encryption::encrypt_file(&mut source, &mut dest, &key).unwrap();
                pb.finish_and_clear();
                let _ = send.send(UploadRequest { filename: destination_filename, data_hash: request.data_hash });
            });
            
            recv.await.expect("Panic in rayon::spawn")
        //}
      //  uploader.blocking_send(UploadRequest { filename: destination_filename, data_hash: request.data_hash }).unwrap();
    //}
}