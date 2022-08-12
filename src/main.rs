pub mod encryption;
pub mod decryption;
pub mod sharding;
pub mod swift;
pub mod datastore;
pub mod metadata_file;
pub mod sqlite_cache;
pub mod hash;
pub mod filetype;
pub mod config;

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::os::unix::prelude::MetadataExt;
use std::fs;
use std::fs::File;
use std::sync::mpsc::channel;

use sequoia_openpgp::Cert;
use sequoia_openpgp::parse::Parse;
use walkdir::WalkDir;

use datastore::DataStore;
use config::BackupConfig;
use sharding::ShardedChannel;
use swift::Bucket;
use sqlite_cache::Cache;
use filetype::FileType;

extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate rmp_serde as rmps;

#[derive(Debug)]
struct UploadRequest {
    filename: std::path::PathBuf,
    data_hash: String,
}

fn create_hash_workers(
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

fn create_upload_workers(stores: Arc<Vec<DataStore>>, data_cache: &PathBuf, key: &Cert) -> (ShardedChannel<UploadRequest>, std::sync::mpsc::Receiver<()>) {
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

fn run_backup(config: BackupConfig) {
    Cache::init();

    let (metadata_tx, metadata_rx) = channel();
    let (hash_tx, hash_rx) = crossbeam_channel::unbounded();

    thread::spawn(move|| {
        for entry in WalkDir::new(config.source) {
            hash_tx.send(entry.unwrap()).unwrap();
        }
    });

    let stores = Arc::new(config.stores);


    let sender = {
        let key = Cert::from_file(config.key_file).unwrap();
        let (upload_channel, s) = create_upload_workers(stores.clone(), &config.data_cache, &key);

        create_hash_workers(hash_rx, metadata_tx, &upload_channel, &stores, config.hmac_secret);
        s
    };

    metadata_file::write_metadata_file(&config.metadata_cache, metadata_rx, stores.clone());

    if sender.recv().is_ok() {
        println!("Unexpected result")
    }

    Cache::cleanup();
}


fn main() {
    let content = std::fs::read_to_string("backup.toml").unwrap();
    let config: BackupConfig = toml::from_str(&content).unwrap();

    run_backup(config);
}
