pub mod encryption;
pub mod decryption;
pub mod swift;

use std::sync::mpsc::Receiver;
use std::thread;
use std::os::unix::prelude::MetadataExt;
use std::io;
use std::fs;
use std::path::Path;
use rusqlite::{Connection, Result, Error};
use ignore::{WalkBuilder, WalkState};
use sha2::{Sha256, Digest};
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


#[derive(Debug, Deserialize, Serialize)]
enum FileType {
    FILE,
    SYMLINK,
    DIRECTORY,
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


fn create_database() -> Result<Connection, Error> {
    return Connection::open("cache.db");
}

fn init_database(connection: &Connection) {
    connection.execute("CREATE TABLE IF NOT EXISTS fs_hash_cache (fs_hash CHARACTER(128) UNIQUE, data_hash CHARACTER(128) NULL, in_use BOOLEAN);", []).unwrap();
    connection.execute("CREATE TABLE IF NOT EXISTS uploaded_objects (data_hash TEXT UNIQUE, encrypted_md5 TEXT);", []).unwrap();
}

fn mark_used_and_lookup_hash(connection: &Connection, filename: &str) -> Result<Option<String>, rusqlite::Error> {
    let mut stmt = connection.prepare("INSERT INTO fs_hash_cache (fs_hash, in_use) VALUES(?, true) ON CONFLICT(fs_hash) do UPDATE set in_use = true RETURNING data_hash").unwrap();
    let mut rows = stmt.query([filename]).unwrap();
    let row = rows.next();
    return match row {
        Ok(Some(r)) => {
            let hash: Result<Option<String>> = r.get(0);
            //let foo = hash.map(|s| Some(s));
            hash
        },
        Ok(None) => Ok(None),
        Err(x) => Err(x),
    };
}

fn is_data_in_cold_storage(connection: &Connection, data_hash: &String) -> Result<bool, rusqlite::Error> {
    let mut stmt = connection.prepare("SELECT EXISTS(SELECT 1 FROM uploaded_objects WHERE data_hash = ?)").unwrap();
    let mut rows = stmt.query([data_hash]).unwrap();
    let row = rows.next();
    return match row {
        Ok(Some(r)) => {
            let hash: Result<u32> = r.get(0);
            hash.map(|x| x == 1)
        },
        Ok(None) => Ok(false),
        Err(x) => Err(x),
    };
}

fn set_data_in_cold_storage(connection: &Connection, hash: &String) -> Result<usize, rusqlite::Error> {
    let mut stmt = connection.prepare("INSERT INTO uploaded_objects VALUES (?, '')").unwrap();
    return stmt.execute([hash]);
}



fn set_data_hash(connection: &Connection, metadata_hash: &str, data_hash: &str) -> Result<usize, rusqlite::Error> {
    let mut stmt = connection.prepare("UPDATE fs_hash_cache set data_hash = ? where fs_hash = ?").unwrap();
    return stmt.execute([data_hash, metadata_hash]);
}

fn hash_file(filename: &str) -> String {
    let mut hasher = Sha256::new();
    let mut file = fs::File::open(filename).unwrap();
    io::copy(&mut file, &mut hasher).unwrap();
    let digest = hasher.finalize();
    return format!("{:X}", digest);
}

fn upload_file(filename: &str, hash: &str) -> Result<(), Error> {
    print!("uploading {:?} to {:?}", filename, hash);
    // todo: encryption
    // upload
    return Ok(())
}

// fn upload_file2(os: Cloud, container: &str, key: &str, source: File) {
//     let os = openstack::Cloud::from_env()
//         .expect("Failed to create an identity provider from the environment");

//     //let container_name = "bucket";
//     //let container = os.get_container(&container_name).expect("Cannot get a container");  

//     os.create_object(container, key, source);
// }

fn process_file(connection: &Connection, bucket: swift::Bucket, filename: &str, metadata_hash: &str) -> Option<String> {
    let res = mark_used_and_lookup_hash(&connection, metadata_hash);
    match res {
        Ok(Some(data_hash)) => {
            return Some(data_hash);
        }
        Ok(None) => { // find hash 
           let data_hash = hash_file(filename);
            set_data_hash(&connection, metadata_hash, &data_hash).unwrap();
            if !is_data_in_cold_storage(&connection, &data_hash).unwrap() {
                let file = fs::File::open(filename).unwrap();
                let key = format!("data/{}", data_hash);
                match bucket.upload(&key, file) {
                    Ok(_) => {
                        set_data_in_cold_storage(&connection, &data_hash).unwrap();
                    },
                    _ => {}
                }
            }
            return Some(data_hash)
        }
        Err(_) => { // give up 
            return None;
        }
    }
}

fn generate_metadata_hash(len: u64, mtime: i64, path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(len.to_ne_bytes());
    hasher.update(mtime.to_ne_bytes());
    hasher.update(path.as_os_str().as_bytes());
    return format!("{:X}", hasher.finalize());
}

fn filetype_from(t: fs::FileType) -> Option<FileType> {
    if t.is_dir() {
        return Some(FileType::DIRECTORY);
    } else if t.is_file() {
        return Some(FileType::FILE);
    } else if t.is_symlink() {
        return Some(FileType::SYMLINK)
    }
    return None;
}

fn write_metadata_file(rx: Receiver<FileMetadata>) {
    let mut vec = Vec::new();

    while let Ok(msg) = rx.recv() {
        vec.push(msg);
    }

    let data = FileData {
        data: vec,
    };

    let destination = File::create("test.dat").unwrap();

    data.serialize(&mut Serializer::new(destination)).unwrap();
}


fn list_objects() {
    let os = openstack::Cloud::from_env()
        .expect("Failed to create an identity provider from the environment");

    let container_name = "bucket";
    let container = os.get_container(&container_name).expect("Cannot get a container");

    println!("Found container with Name = {}, Number of object = {}",
        container.name(),
        container.object_count()
    );

    let objects: Vec<openstack::object_storage::Object> = container
        .find_objects()
        .with_limit(10)
        .all()
        .expect("cannot list objects");

    println!("first 10 objects");
    for o in objects {
        println!("Name = {}, Bytes = {}, Hash = {}",
            o.name(),
            o.bytes(),
            o.hash().as_ref().unwrap_or(&String::from("")),
        );
    }
}


fn main() {
    let connection = create_database().unwrap();

    init_database(&connection);

    list_objects();
//    return;

    // let par = WalkBuilder::new("/home/david/local/cad")
    //     .hidden(false)
    //     .build_parallel();

    let (tx, rx) = channel();

    thread::spawn(move|| {
        for entry in WalkDir::new("/home/david/local/cad") {
            let con = create_database().unwrap();
            let tx = tx.clone();

            let os = openstack::Cloud::from_env()
                .expect("Failed to create an identity provider from the environment");
            let bucket = swift::Bucket::new(&os, "bucket");
            let dir_entry = entry.unwrap();
            let metadata = dir_entry.metadata().unwrap();

            let mut destination: Option<String> = None;
            let mut data_hash: Option<String> = None;

            let file_type = filetype_from(dir_entry.file_type());

            let path = dir_entry.path().to_string_lossy();

            match file_type {
                Some(FileType::FILE) => {
                    let metadata_hash = generate_metadata_hash(metadata.len(), metadata.mtime(), dir_entry.path());
                    data_hash = process_file(&con, bucket, &path, &metadata_hash);    
                }
                Some(FileType::SYMLINK) => {
                    destination = Some(std::fs::read_link(dir_entry.path()).unwrap().to_string_lossy().to_string());
                }
                _ => {
                }
            }

            match file_type {
                Some(t) => {
                    tx.send(FileMetadata {
                        name: path.to_string(),
                        mtime: metadata.mtime(),
                        mode: metadata.mode(),
                        xattr: HashMap::new(),
                        ttype: t,
                        destination,
                        data_hash,
                    }).unwrap();
                }
                _ => {}
            }
        }
    });

    write_metadata_file(rx);
    
    encryption::do_encryption().unwrap();

}
