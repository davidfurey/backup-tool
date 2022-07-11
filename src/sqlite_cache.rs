use std::fs::Metadata;
use std::path::Path;

use crate::datastore;
use crate::hash;

use std::os::unix::prelude::MetadataExt;
use rusqlite::{Connection, Result, Error};
use datastore::DataStore;

pub struct Cache {
  connection: Connection,
}

impl Cache {
  pub fn new<'b>() -> Cache {
    Cache {
      connection: Cache::db_connect().unwrap(),
    }
  }

  pub fn init() {
    let connection = Cache::db_connect().unwrap();
    connection.execute("CREATE TABLE IF NOT EXISTS fs_hash_cache (fs_hash CHARACTER(128) UNIQUE, data_hash CHARACTER(128) NULL, in_use BOOLEAN);", []).unwrap();
    connection.execute("CREATE TABLE IF NOT EXISTS uploaded_objects (data_hash TEXT UNIQUE, encrypted_md5 TEXT NULL, datastore_id INTEGER);", []).unwrap();
    connection.execute("UPDATE fs_hash_cache set in_use = false;", []).unwrap();
  }

  pub fn cleanup() {
    let connection = Cache::db_connect().unwrap();
    connection.execute("DELETE FROM fs_hash_cache WHERE in_use = false;", []).unwrap();
  }

  fn db_connect() -> Result<Connection, Error> {
    return Connection::open("cache.db");
  }

  pub fn get_hash(&self, path: &Path, metadata: &Metadata, hmac_secret: &str) -> Result<String, rusqlite::Error> {
    let metadata_hash = hash::metadata(metadata.len(), metadata.mtime(), path);

    let res = self.mark_used_and_lookup_hash(&metadata_hash);
    res.map(|v| {
        v.unwrap_or_else(|| {
            let data_hash = hash::data(path, hmac_secret);
            self.set_data_hash(&metadata_hash, &data_hash).unwrap();
            data_hash
        })
    })
  }

  fn mark_used_and_lookup_hash(&self, filename: &str) -> Result<Option<String>, rusqlite::Error> {
    let mut stmt = self.connection.prepare("INSERT INTO fs_hash_cache (fs_hash, in_use) VALUES(?, true) ON CONFLICT(fs_hash) do UPDATE set in_use = true RETURNING data_hash").unwrap();
    let mut rows = stmt.query([filename]).unwrap();
    let row = rows.next();
    match row {
        Ok(Some(r)) => r.get(0),
        Ok(None) => Ok(None),
        Err(x) => Err(x),
    }
  }
  
  pub fn is_data_in_cold_storage(&self, data_hash: &String, stores: &Vec<DataStore>) -> Result<bool, rusqlite::Error> {
    let mut stmt = self.connection.prepare("SELECT 1 FROM uploaded_objects WHERE data_hash = ? and datastore_id = ?").unwrap();
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
  
  pub fn set_data_in_cold_storage(&self, hash: &str, md5_hash: &str, datastore_id: i32) -> Result<usize, rusqlite::Error> {
    let mut stmt = self.connection.prepare("INSERT INTO uploaded_objects VALUES (?, ?, ?)").unwrap();
    return stmt.execute([hash, md5_hash, datastore_id.to_string().as_str()]);
  }
  
  
  fn set_data_hash(&self, metadata_hash: &str, data_hash: &str) -> Result<usize, rusqlite::Error> {
    let mut stmt = self.connection.prepare("UPDATE fs_hash_cache set data_hash = ? where fs_hash = ?").unwrap();
    return stmt.execute([data_hash, metadata_hash]);
  }  
}