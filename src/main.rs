pub mod encryption;
pub mod decryption;
pub mod sharding;
pub mod swift;

use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::thread;
use std::os::unix::prelude::MetadataExt;
use std::io;
use std::fs;
use std::path::Path;
use rusqlite::{Connection, Result, Error};
use sha2::{Sha256, Digest};
use hmac::{Hmac, Mac};
use sharding::ShardedChannel;
use std::os::unix::ffi::OsStrExt;
use std::sync::mpsc::channel;
use std::collections::HashMap;
use std::fs::File;
use walkdir::WalkDir;

extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate rmp_serde as rmps;

use serde::{Serialize};
use rmps::{Serializer};


#[derive(Debug, Deserialize, Serialize, Clone)]
enum FileType {
    FILE,
    SYMLINK,
    DIRECTORY,
}

#[derive(Debug)]
struct UploadRequest {
    filename: String,
    data_hash: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct FileMetadata {
    name: String,
    mtime: i64,
    mode: u32,
    xattr: HashMap<String, String>,
    ttype: FileType,
    destination: Option<String>,
    data_hash: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct FileData {
    data: Vec<FileMetadata>,
}

struct DataStore {
    id: i32,
}

fn db_connect() -> Result<Connection, Error> {
    return Connection::open("cache.db");
}

fn db_init(connection: &Connection) {
    connection.execute("CREATE TABLE IF NOT EXISTS fs_hash_cache (fs_hash CHARACTER(128) UNIQUE, data_hash CHARACTER(128) NULL, in_use BOOLEAN);", []).unwrap();
    connection.execute("CREATE TABLE IF NOT EXISTS uploaded_objects (data_hash TEXT UNIQUE, encrypted_md5 TEXT NULL, datastore_id INTEGER);", []).unwrap();
}

fn mark_used_and_lookup_hash(connection: &Connection, filename: &str) -> Result<Option<String>, rusqlite::Error> {
    let mut stmt = connection.prepare("INSERT INTO fs_hash_cache (fs_hash, in_use) VALUES(?, true) ON CONFLICT(fs_hash) do UPDATE set in_use = true RETURNING data_hash").unwrap();
    let mut rows = stmt.query([filename]).unwrap();
    let row = rows.next();
    match row {
        Ok(Some(r)) => r.get(0),
        Ok(None) => Ok(None),
        Err(x) => Err(x),
    }
}

fn is_data_in_cold_storage(connection: &Connection, data_hash: &String, stores: &Vec<DataStore>) -> Result<bool, rusqlite::Error> {
    let mut stmt = connection.prepare("SELECT 1 FROM uploaded_objects WHERE data_hash = ? and datastore_id = ?").unwrap();
    for store in stores {
        let mut rows = stmt.query([data_hash, store.id.to_string().as_str()]).unwrap();
        match rows.next() {
            Ok(None) => return Ok(false),
            Ok(_) => { },
            Err(x) => return Err(x)
        };
    }
    return Ok(true);
}

// fn set_data_in_cold_storage(connection: &Connection, hash: &str) -> Result<usize, rusqlite::Error> {
//     let mut stmt = connection.prepare("INSERT OR IGNORE INTO uploaded_objects VALUES (?, NULL)").unwrap();
//     return stmt.execute([hash]);
// }

fn set_data_in_cold_storage(connection: &Connection, hash: &str, md5_hash: &str, datastore_id: i32) -> Result<usize, rusqlite::Error> {
    let mut stmt = connection.prepare("INSERT INTO uploaded_objects VALUES (?, ?, ?)").unwrap();
    return stmt.execute([hash, md5_hash, datastore_id.to_string().as_str()]);
}


// fn update_hash_in_cold_storage(connection: &Connection, hash: &str, md5_hash: &str) -> Result<usize, rusqlite::Error> {
//     let mut stmt = connection.prepare("UPDATE uploaded_objects set encrypted_md5 = ? where data_hash = ?").unwrap();
//     return stmt.execute([hash, md5_hash]);
// }

fn set_data_hash(connection: &Connection, metadata_hash: &str, data_hash: &str) -> Result<usize, rusqlite::Error> {
    let mut stmt = connection.prepare("UPDATE fs_hash_cache set data_hash = ? where fs_hash = ?").unwrap();
    return stmt.execute([data_hash, metadata_hash]);
}

// todo: HMAC secret key
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

fn process_file(connection: &Connection, path: &Path, metadata_hash: &str) -> Result<String, rusqlite::Error> {
    let res = mark_used_and_lookup_hash(&connection, metadata_hash);
    res.map(|v| {
        v.unwrap_or_else(|| {
            let data_hash = hash_file(path);
            set_data_hash(&connection, metadata_hash, &data_hash).unwrap();
            data_hash
        })
    })
}

fn generate_metadata_hash(len: u64, mtime: i64, path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(len.to_ne_bytes());
    hasher.update(mtime.to_ne_bytes());
    hasher.update(path.as_os_str().as_bytes());
    format!("{:X}", hasher.finalize())
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

fn write_metadata_file(metadata_rx: Receiver<FileMetadata>) {
    let mut vec = Vec::new();

    while let Ok(msg) = metadata_rx.recv() {
        vec.push(msg);
    }

    let data = FileData {
        data: vec,
    };

    let destination = File::create("test.dat").unwrap();

    data.serialize(&mut Serializer::new(destination)).unwrap();
}


// fn list_objects() {
//     let os = openstack::Cloud::from_env()
//         .expect("Failed to create an identity provider from the environment");

//     let container_name = "bucket";
//     let container = os.get_container(&container_name).expect("Cannot get a container");

//     println!("Found container with Name = {}, Number of object = {}",
//         container.name(),
//         container.object_count()
//     );

//     let objects: Vec<openstack::object_storage::Object> = container
//         .find_objects()
//         .with_limit(10)
//         .all()
//         .expect("cannot list objects");

//     println!("first 10 objects");
//     for o in objects {
//         println!("Name = {}, Bytes = {}, Hash = {}",
//             o.name(),
//             o.bytes(),
//             o.hash().as_ref().unwrap_or(&String::from("")),
//         );
//     }
// }

fn create_hash_workers(
    hash_rx: crossbeam_channel::Receiver<walkdir::DirEntry>,
    metadata_tx: std::sync::mpsc::Sender<FileMetadata>,
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
    metadata_tx: &std::sync::mpsc::Sender<FileMetadata>,
    upload_tx: &ShardedChannel<UploadRequest>,
    stores: Arc<Vec<DataStore>>,
) {
    let hash_rx = hash_rx.clone();
    let metadata_tx = metadata_tx.clone();
    let upload_tx = upload_tx.clone();

    thread::spawn(move|| {
        let con = db_connect().unwrap();

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
                        &con, 
                        dir_entry.path(),
                        &metadata_hash
                    );

                    d_hash.map(|d_hash| {
                        if !is_data_in_cold_storage(&con, &d_hash, &stores).unwrap() {
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
                metadata_tx.send(FileMetadata {
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
    static mut x: u32 = 1;
    let id = unsafe {
        x += 1;
        x
    };
    thread::spawn(move|| {            
        let con = db_connect().unwrap();
        let os = openstack::Cloud::from_env()
            .expect("Failed to create an identity provider from the environment");
        let bucket = swift::Bucket::new(&os, "bucket");

        while let Ok(request) = upload_rx.recv() {
            print!("Processing upload request: {:?} {} ({})\n", request.filename, request.data_hash, id);
            if is_data_in_cold_storage(&con, &request.data_hash, &stores).unwrap() {
                print!("Skipping duplicate hash {}\n", request.data_hash);
            } else {
                let destination_filename = format!("data/{}.gpg", request.data_hash);
                {
                    let mut file = fs::File::open(request.filename).unwrap();
                    let mut destination = File::create(&destination_filename).unwrap();
                    encryption::encrypt_file(&mut file, &mut destination).unwrap();
                }
                let encrypted_file = fs::File::open(&destination_filename).unwrap();
                let key = format!("data/{}", request.data_hash);

                // todo: actually check and upload for multiple buckets
                match bucket.upload(&key, encrypted_file) {
                    Ok(_) => {
                        set_data_in_cold_storage(&con, request.data_hash.as_str(), "", 1).unwrap();
                    },
                    _ => {}
                }
            }
        }
        print!("Done with uploads\n");
    });
}

// fn create_upload_workers(upload_rx: crossbeam_channel::Receiver<UploadRequest>) {
//     for _n in 0..4 {
//         create_upload_worker(&upload_rx);
//     }
// }
// fn create_upload_worker(upload_rx: &crossbeam_channel::Receiver<UploadRequest>) {
//     let upload_rx = upload_rx.clone();
//     thread::spawn(move|| {            
//         let con = db_connect().unwrap();
//         let os = openstack::Cloud::from_env()
//             .expect("Failed to create an identity provider from the environment");
//         let bucket = swift::Bucket::new(&os, "bucket");

//         while let Ok(request) = upload_rx.recv() {
//             print!("Processing upload request: {:?}\n", request.filename);
//             let destination_filename = format!("data/{}.gpg", request.data_hash);
//             {
//                 let mut file = fs::File::open(request.filename).unwrap();
//                 let mut destination = File::create(&destination_filename).unwrap();
//                 encryption::encrypt_file(&mut file, &mut destination).unwrap();
//             }
//             let encrypted_file = fs::File::open(&destination_filename).unwrap();
//             let key = format!("data/{}", request.data_hash);
//             match bucket.upload(&key, encrypted_file) {
//                 Ok(_) => {
//                     set_data_in_cold_storage(&con, request.data_hash.as_str(), "", 1).unwrap();
//                 },
//                 _ => {}
//             }
//         }
//         print!("Done with uploads\n");
//     });
// }

fn main() {
    let connection = db_connect().unwrap();

    db_init(&connection);

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
    });

    let stores = Arc::new(stores);

    let upload_channel = create_upload_workers(stores.clone());

    create_hash_workers(hash_rx, metadata_tx, &upload_channel, stores);
    
    drop(upload_channel);
    drop(connection);
//    drop(metadata_rx);
//    drop(metadata_tx);
//    drop(hash_tx);
//    drop(hash_rx);
//    drop(stores);

    write_metadata_file(metadata_rx);

    // todo: wait for all threads to finish
    thread::sleep(std::time::Duration::from_millis(30000));
}
