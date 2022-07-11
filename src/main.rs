pub mod encryption;
pub mod decryption;
pub mod sharding;
pub mod swift;
pub mod datastore;
pub mod metadata_file;
pub mod sqlite_cache;
pub mod hash;
pub mod filetype;

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
            print!("Processing hash request: {:?}\n", dir_entry.file_name());
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

fn create_upload_workers(stores: Arc<Vec<DataStore>>, data_cache: &PathBuf, key: &Cert) -> ShardedChannel<UploadRequest> {
    sharding::ShardedChannel::new(4, |f| {
        create_upload_worker(f, stores.clone(), data_cache, key);
    })
}

fn create_upload_worker(upload_rx: std::sync::mpsc::Receiver<UploadRequest>, stores: Arc<Vec<DataStore>>, data_cache: &PathBuf, key: &Cert) {
    static mut UPLOAD_ID_COUNT: u32 = 1;
    let id = unsafe {
        UPLOAD_ID_COUNT += 1;
        UPLOAD_ID_COUNT
    };
    let key = key.clone();
    let data_cache = data_cache.clone();
    thread::spawn(move|| {
        let buckets: Vec<(&DataStore, Bucket)> = stores.iter().map(|store| {
            (store, store.init())
        }).collect();
        let cache = Cache::new();

        while let Ok(request) = upload_rx.recv() {
            print!("Processing upload request: {:?} {} ({})\n", request.filename, request.data_hash, id);
            if cache.is_data_in_cold_storage(&request.data_hash, &stores).unwrap() {
                print!("Skipping duplicate hash {}\n", request.data_hash);
            } else {
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


struct BackupConfig {
    source: PathBuf,
    data_cache: PathBuf,
    metadata_cache: PathBuf,
    stores: Vec<DataStore>,
    hmac_secret: String,
    key_file: PathBuf,
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

    {
        let key = Cert::from_file(config.key_file).unwrap();
        let upload_channel = create_upload_workers(stores.clone(), &config.data_cache, &key);

        create_hash_workers(hash_rx, metadata_tx, &upload_channel, &stores, config.hmac_secret);
    }

    metadata_file::write_metadata_file(&config.metadata_cache, metadata_rx, stores.clone());

    // todo: wait for all threads to finish
    thread::sleep(std::time::Duration::from_millis(30000));

    Cache::cleanup();
}


fn main() {
    let mut stores = Vec::new();

    stores.push(DataStore {
        id: 1,
        data_container: "bucket".to_string(),
        metadata_container: "metadata".to_string(),
        data_prefix: "data/".to_string(),
        metadata_prefix: "metadata/".to_string(),
    });

    let config = BackupConfig {
        source: PathBuf::from("/home/david/local/cad"),
        stores,
        data_cache: PathBuf::from("/tmp/data"),
        metadata_cache: PathBuf::from("/tmp/metadata"),
        hmac_secret: "test2".to_string(),
        key_file: PathBuf::from("priv.key"),
    };

    run_backup(config);
}
