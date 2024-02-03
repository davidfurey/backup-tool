use std::thread;
use std::os::unix::prelude::MetadataExt;
use crate::{metadata_file::{self, FileMetadata}, upload_worker, sharding, datastore, sqlite_cache::AsyncCache, filetype, hash};
use datastore::DataStore;
use indicatif::{MultiProgress, ProgressStyle, ProgressBar};
use log::trace;
use sharding::ShardedChannel;
use filetype::FileType;
use tokio::{runtime::Handle, task::JoinHandle};
use upload_worker::UploadRequest;


pub fn create_hash_workers(
    hash_rx: crossbeam_channel::Receiver<walkdir::DirEntry>,
    metadata_tx: std::sync::mpsc::Sender<metadata_file::FileMetadata>,
    upload_tx: ShardedChannel<UploadRequest>,
    stores:  Vec<DataStore>,
    hmac_secret: String,
    m: &MultiProgress,
    cache: AsyncCache
) {
    let spinner_style = ProgressStyle::with_template("{prefix:.bold.dim} {spinner} {wide_msg}")
        .unwrap()
        .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ");
    let mut i = 0;
    let length = 4;
    for _n in 0..4 {
        let pb = m.add(ProgressBar::new_spinner());
        pb.set_style(spinner_style.clone());
        pb.set_prefix(format!("[Hasher {}/{}]", i + 1, length));
        i = i + 1;
        create_hash_worker(&hash_rx, &metadata_tx, &upload_tx, &stores, hmac_secret.clone(), pb, cache.clone());
     }
}

pub async fn hash_work(dir_entry: walkdir::DirEntry, cache: &AsyncCache, stores: &Vec<DataStore>, hmac_secret: &String) -> (Option<UploadRequest>, Option<FileMetadata>) {
    let file_type: Option<FileType> = FileType::from(dir_entry.file_type());
    let mut destination: Option<String> = None;
    let mut data_hash: Option<String> = None;
    let metadata = dir_entry.metadata().unwrap();
    let mut uploadRequest: Option<UploadRequest> = None;
    match file_type {
        Some(FileType::FILE) => {
            let cached_d_hash = cache.try_get_hash(dir_entry.path(), &metadata).await.unwrap();
            let d_hash = match cached_d_hash {
                Some(h) => h,
                None => {
                    let hms = hmac_secret.clone();
                    let de = dir_entry.path().to_owned().clone();
                    let (send, recv) = tokio::sync::oneshot::channel();
                    rayon::spawn(move || {
                        println!("Rayon spawned"); // never runs?
                        let res = hash::data(&de, &hms);
                        let _ = send.send(res);
                    });
                    println!("Rayon threads: {:?}", rayon::current_num_threads());
                    println!("Rayon max threads: {:?}", rayon::max_num_threads());
                    let res = recv.await.expect("Panic in rayon::spawn");
                    // let res = tokio::task::spawn_blocking(move || { // should probs use a different thread pool for this
                    //     hash::data(&de, &hms)
                    // }).await.unwrap();
                    
                    let metadata_hash = hash::metadata(metadata.len(), metadata.mtime(), dir_entry.path());
                    cache.set_data_hash(&metadata_hash, &res).await.unwrap();
                    res
                }
            };
            
            if !cache.is_data_in_cold_storage(&d_hash, &stores).await.unwrap() {
                trace!("Sending {:?} to upload queue\n", dir_entry.file_name());
                uploadRequest = Some(UploadRequest {
                    filename: dir_entry.path().to_path_buf(),  
                    data_hash: d_hash.clone(),
                });
            } else {
                trace!("Skipping {:?} ({:?} already uploaded)\n", dir_entry.file_name(), d_hash);
            }
            data_hash = Some(d_hash);
        }
        Some(FileType::SYMLINK) => {
            destination = Some(std::fs::read_link(dir_entry.path()).unwrap().to_string_lossy().to_string());
            trace!("Symbolic link from {:?} to {:?}", dir_entry.path().to_string_lossy().to_string(), destination);
        }
        _ => {}
    }

    let fileMetadata = file_type.map(|ttype| {
        metadata_file::FileMetadata {
            name: dir_entry.path().to_string_lossy().to_string(),
            mtime: metadata.mtime(),
            mode: metadata.mode(),
            ttype: ttype,
            destination,
            data_hash,
        }
    });
    (uploadRequest, fileMetadata)
}

fn create_hash_worker(
    hash_rx: &crossbeam_channel::Receiver<walkdir::DirEntry>,
    metadata_tx: &std::sync::mpsc::Sender<metadata_file::FileMetadata>,
    upload_tx: &ShardedChannel<UploadRequest>,
    stores: &Vec<DataStore>,
    hmac_secret: String,
    pb: ProgressBar,
    cache: AsyncCache
) {
    let hash_rx = hash_rx.clone();
    let metadata_tx = metadata_tx.clone();
    let upload_tx = upload_tx.clone();

    let stores = stores.to_vec();
    //thread::spawn(move|| {
    tokio::task::spawn_blocking(move || {
        while let Ok(dir_entry) = hash_rx.recv() {
            pb.inc(1);
            pb.set_message(format!("{}", dir_entry.file_name().to_string_lossy()));
            let file_type: Option<FileType> = FileType::from(dir_entry.file_type());
            let mut destination: Option<String> = None;
            let mut data_hash: Option<String> = None;
            let metadata = dir_entry.metadata().unwrap();
            match file_type {
                Some(FileType::FILE) => {
                    let d_hash = Handle::current().block_on(cache.get_hash(dir_entry.path(), &metadata, hmac_secret.as_str())).unwrap();

                    if !Handle::current().block_on(cache.is_data_in_cold_storage(&d_hash, &stores)).unwrap() {
                        trace!("Sending {:?} to upload queue\n", dir_entry.file_name());
                        upload_tx.send(UploadRequest {
                            filename: dir_entry.path().to_path_buf(),  
                            data_hash: d_hash.clone(),
                        }, d_hash.as_str()).unwrap();
                    } else {
                        trace!("Skipping {:?} ({:?} already uploaded)\n", dir_entry.file_name(), d_hash);
                    }
                    data_hash = Some(d_hash);
                }
                Some(FileType::SYMLINK) => {
                    destination = Some(std::fs::read_link(dir_entry.path()).unwrap().to_string_lossy().to_string());
                    trace!("Symbolic link from {:?} to {:?}", dir_entry.path().to_string_lossy().to_string(), destination);
                }
                _ => {}
            }

            file_type.map(|ttype| {
                metadata_tx.send(metadata_file::FileMetadata {
                    name: dir_entry.path().to_string_lossy().to_string(),
                    mtime: metadata.mtime(),
                    mode: metadata.mode(),
                    ttype: ttype,
                    destination,
                    data_hash,
                }).unwrap();
            });
        }
    });
}