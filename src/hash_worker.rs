use std::sync::Arc;
use std::thread;
use std::os::unix::prelude::MetadataExt;
use crate::{metadata_file, upload_worker, sharding, datastore, sqlite_cache, filetype};
use datastore::DataStore;
use sharding::ShardedChannel;
use sqlite_cache::Cache;
use filetype::FileType;
use upload_worker::UploadRequest;


pub fn create_hash_workers(
    hash_rx: crossbeam_channel::Receiver<walkdir::DirEntry>,
    metadata_tx: std::sync::mpsc::Sender<metadata_file::FileMetadata>,
    upload_tx: &ShardedChannel<UploadRequest>,
    stores:  &Arc<Vec<DataStore>>,
    hmac_secret: String,
) {
    for _n in 0..4 {
        create_hash_worker(&hash_rx, &metadata_tx, upload_tx, stores.clone(), hmac_secret.clone());
     } 
}

fn create_hash_worker(
    hash_rx: &crossbeam_channel::Receiver<walkdir::DirEntry>,
    metadata_tx: &std::sync::mpsc::Sender<metadata_file::FileMetadata>,
    upload_tx: &ShardedChannel<UploadRequest>,
    stores: Arc<Vec<DataStore>>,
    hmac_secret: String,
) {
    let hash_rx = hash_rx.clone();
    let metadata_tx = metadata_tx.clone();
    let upload_tx = upload_tx.clone();

    thread::spawn(move|| {
        let cache = Cache::new();

        while let Ok(dir_entry) = hash_rx.recv() {
            let file_type = FileType::from(dir_entry.file_type());
            let mut destination: Option<String> = None;
            let mut data_hash: Option<String> = None;
            let metadata = dir_entry.metadata().unwrap();
            match file_type {
                Some(FileType::FILE) => {
                    let d_hash = cache.get_hash(dir_entry.path(), &metadata, hmac_secret.as_str()).unwrap();

                    if !cache.is_data_in_cold_storage(&d_hash, &stores).unwrap() {
                        upload_tx.send(UploadRequest {
                            filename: dir_entry.path().to_path_buf(),  
                            data_hash: d_hash.clone(),
                        }, d_hash.as_str()).unwrap();
                    } else {
                        print!("Skipping {:?} ({:?} already uploaded)\n", dir_entry.file_name(), d_hash);
                    }
                    data_hash = Some(d_hash);
                }
                Some(FileType::SYMLINK) => {
                    destination = Some(std::fs::read_link(dir_entry.path()).unwrap().to_string_lossy().to_string());
                    println!("Symbolic link from {:?} to {:?}", dir_entry.path().to_string_lossy().to_string(), destination);
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
        print!("Done with hashing\n");
    });
}