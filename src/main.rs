pub mod encryption;
pub mod decryption;
pub mod sharding;
pub mod swift;
pub mod datastore;
pub mod metadata_file;
pub mod sqlite_cache;

use datastore::DataStore;

use std::sync::Arc;
use std::thread;
use std::os::unix::prelude::MetadataExt;
use std::io;
use std::fs;
use std::path::Path;
use rusqlite::Result;
use sha2::{Sha256, Digest};
use hmac::{Hmac, Mac};
use sharding::ShardedChannel;
use std::os::unix::ffi::OsStrExt;
use std::sync::mpsc::channel;
use std::collections::HashMap;
use std::fs::File;
use walkdir::WalkDir;
use swift::Bucket;
use sqlite_cache::Cache;

extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate rmp_serde as rmps;

use serde::Serialize;


#[derive(Debug, Deserialize, Serialize, Clone)]
pub enum FileType {
    FILE,
    SYMLINK,
    DIRECTORY,
}

#[derive(Debug)]
struct UploadRequest {
    filename: String,
    data_hash: String,
}

fn generate_metadata_hash(len: u64, mtime: i64, path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(len.to_ne_bytes());
    hasher.update(mtime.to_ne_bytes());
    hasher.update(path.as_os_str().as_bytes());
    format!("{:X}", hasher.finalize())
}

fn hash_file(path: &Path) -> String {
    type HmacSha256 = Hmac<Sha256>;
    let mut hasher = HmacSha256::new_from_slice(b"my secret and secure key")
        .expect("HMAC can take key of any size");
    //let mut hasher = Sha256::new();
    let mut file = fs::File::open(path).unwrap();
    io::copy(&mut file, &mut hasher).unwrap();
    let digest = hasher.finalize().into_bytes();
    return format!("{:X}", digest);
}

fn process_file(cache: &Cache, path: &Path, metadata_hash: &str) -> Result<String, rusqlite::Error> {
    let res = cache.mark_used_and_lookup_hash(metadata_hash);
    res.map(|v| {
        v.unwrap_or_else(|| {
            let data_hash = hash_file(path);
            cache.set_data_hash(metadata_hash, &data_hash).unwrap();
            data_hash
        })
    })
}

fn filetype_from(t: fs::FileType) -> Option<FileType> {
    if t.is_dir() {
        Some(FileType::DIRECTORY)
    } else if t.is_file() {
        Some(FileType::FILE)
    } else if t.is_symlink() {
        Some(FileType::SYMLINK)
    } else {
        None
    }
}


fn create_hash_workers(
    hash_rx: crossbeam_channel::Receiver<walkdir::DirEntry>,
    metadata_tx: std::sync::mpsc::Sender<metadata_file::FileMetadata>,
    upload_tx: &ShardedChannel<UploadRequest>,
    stores:  Arc<Vec<DataStore>>,
) {
    //let stores = Arc::new(stores);
    for _n in 0..4 {
        create_hash_worker(&hash_rx, &metadata_tx, upload_tx, stores.clone());
     } 
}

fn create_hash_worker(
    hash_rx: &crossbeam_channel::Receiver<walkdir::DirEntry>,
    metadata_tx: &std::sync::mpsc::Sender<metadata_file::FileMetadata>,
    upload_tx: &ShardedChannel<UploadRequest>,
    stores: Arc<Vec<DataStore>>,
) {
    let hash_rx = hash_rx.clone();
    let metadata_tx = metadata_tx.clone();
    let upload_tx = upload_tx.clone();

    thread::spawn(move|| {
        let cache = Cache::new();

        while let Ok(dir_entry) = hash_rx.recv() {
            print!("Processing hash request: {:?}\n", dir_entry.file_name());
            let file_type = filetype_from(dir_entry.file_type());
            let mut destination: Option<String> = None;
            let mut data_hash: Option<String> = None;
            let metadata = dir_entry.metadata().unwrap();
            match file_type {
                Some(FileType::FILE) => {
                    let metadata_hash = generate_metadata_hash(metadata.len(), metadata.mtime(), dir_entry.path());
                    let filename = dir_entry.path().to_string_lossy();
                    let d_hash = process_file(
                        &cache, 
                        dir_entry.path(),
                        &metadata_hash
                    );

                    d_hash.map(|d_hash| {
                        if !cache.is_data_in_cold_storage(&d_hash, &stores).unwrap() {
                            upload_tx.send(UploadRequest {
                                filename: filename.to_string(),  
                                data_hash: d_hash.clone(),
                            }, d_hash.clone().as_str()).unwrap();
                        }
                        data_hash = Some(d_hash);
                    }).unwrap();
                }
                Some(FileType::SYMLINK) => {
                    destination = Some(std::fs::read_link(dir_entry.path()).unwrap().to_string_lossy().to_string());
                }
                _ => {
                }
            }

            file_type.map(|ttype| {
                metadata_tx.send(metadata_file::FileMetadata {
                    name: dir_entry.path().to_string_lossy().to_string(),
                    mtime: metadata.mtime(),
                    mode: metadata.mode(),
                    xattr: HashMap::new(),
                    ttype: ttype,
                    destination,
                    data_hash,
                }).unwrap();
            });
        }
        print!("Done with hashing\n");
    });
}

fn create_upload_workers(stores: Arc<Vec<DataStore>>) -> ShardedChannel<UploadRequest> {
    sharding::ShardedChannel::new(4, |f| {
        create_upload_worker(f, stores.clone());
    })
}

fn create_upload_worker(upload_rx: std::sync::mpsc::Receiver<UploadRequest>, stores: Arc<Vec<DataStore>>) {
    static mut UPLOAD_ID_COUNT: u32 = 1;
    let id = unsafe {
        UPLOAD_ID_COUNT += 1;
        UPLOAD_ID_COUNT
    };
    thread::spawn(move|| {
        let buckets: Vec<Bucket> = stores.iter().map(|store| {
            store.init()
        }).collect();
        let cache = Cache::new();

        while let Ok(request) = upload_rx.recv() {
            print!("Processing upload request: {:?} {} ({})\n", request.filename, request.data_hash, id);
            if cache.is_data_in_cold_storage(&request.data_hash, &stores).unwrap() {
                print!("Skipping duplicate hash {}\n", request.data_hash);
            } else {
                let destination_filename = format!("data/{}.gpg", request.data_hash);
                {
                    let mut source = fs::File::open(request.filename).unwrap();
                    let mut dest = File::create(&destination_filename).unwrap();
                    encryption::encrypt_file(&mut source, &mut dest).unwrap();
                }

                for bucket in buckets.as_slice() {
                    let encrypted_file = fs::File::open(&destination_filename).unwrap();
                    let key = format!("data/{}", request.data_hash);
                    match bucket.upload(&key, encrypted_file) {
                        Ok(_) => {
                            cache.set_data_in_cold_storage(request.data_hash.as_str(), "", 1).unwrap();
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


fn main() {
    Cache::init();

// todo:
// first of all, delete anything from uploaded where md5sum_hash is null
// then delete from fs_hash_cache where in_use is false

    let (metadata_tx, metadata_rx) = channel();
    let (hash_tx, hash_rx) = crossbeam_channel::unbounded();
    //let (upload_tx, upload_rx) = crossbeam_channel::unbounded();

    thread::spawn(move|| {
        for entry in WalkDir::new("/home/david/local/cad") {
            hash_tx.send(entry.unwrap()).unwrap();
        }
    });

    let mut stores = Vec::new();

    stores.push(DataStore {
        id: 1,
        container: "bucket".to_string(),
    });

    let stores = Arc::new(stores);

    let upload_channel = create_upload_workers(stores.clone());

    create_hash_workers(hash_rx, metadata_tx, &upload_channel, stores);
    
    drop(upload_channel);

    metadata_file::write_metadata_file(metadata_rx);

    // todo: wait for all threads to finish
    thread::sleep(std::time::Duration::from_millis(30000));
}
