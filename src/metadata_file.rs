use serde::Serialize;
extern crate rmp_serde as rmps;
use crate::filetype;
use filetype::FileType;
use sqlx::Executor;
use sqlx::SqlitePool;
use sqlx;
use sqlx::Row;

#[derive(Debug, Deserialize, Serialize)]
pub struct FileData {
    pub data: Vec<FileMetadata>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct FileMetadata {
    pub uid : i64,
    pub name: String,
    pub mtime: i64,
    pub mode: u32,
    pub ttype: FileType,
    pub destination: Option<String>,
    pub data_hash: Option<String>,
}

pub struct MetadataWriter {
  pool: SqlitePool
}

pub struct MetadataReader {
  pool: SqlitePool
}

use std::path::PathBuf;

impl MetadataReader {
  pub async fn new<'b>(filename: PathBuf) -> MetadataReader {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
      .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
      .read_only(true)
      .filename(filename);
    let reader = MetadataReader {
      pool: SqlitePool::connect_with(options).await.unwrap()
    };
    let result = reader.pool.fetch_one(sqlx::query("SELECT value FROM metadata where key = 'version';")).await.unwrap();
    let version: String = result.get(0);
    if version != "0" {
      panic!("Version is not supported")
    }
    reader
  }

  pub async fn read_metadata(&self, key: &str) -> String {
    let result = self.pool.fetch_one(
      sqlx::query("SELECT value FROM metadata where key = ?;")
        .bind(key)
    ).await.unwrap();
    result.get(0)
  }

  pub async fn read(&self) -> futures_core::stream::BoxStream<FileMetadata> {
    use futures::StreamExt;
    let query = sqlx::query("SELECT id, name, mtime, mode, ttype, destination, data_hash from files order by id asc;");
    Box::pin(self.pool.fetch(query).map(|z| {
      let row = z.unwrap();
      FileMetadata {
        uid: row.get(0),
        name: row.get(1),
        mtime: row.get(2),
        mode: row.get(3),
        ttype: row.get(4),
        destination: row.get(5),
        data_hash: row.get(6)
      }
    }))
  }
}

impl MetadataWriter {
  pub async fn new<'b>(filename: PathBuf) -> MetadataWriter {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
      .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
      .create_if_missing(true)
      .filename(filename);
    let metadata_file = MetadataWriter {
      pool: SqlitePool::connect_with(options).await.unwrap()
    };
    metadata_file.pool.execute(sqlx::query("CREATE TABLE files (id INTEGER PRIMARY KEY, name TEXT, mtime INTEGER, mode INTEGER, ttype STRING, destination STRING NULL, data_hash STRING NULL);")).await.unwrap();
    metadata_file.pool.execute(sqlx::query("CREATE TABLE metadata (key TEXT, value TEXT);")).await.unwrap();
    metadata_file.pool.execute(sqlx::query("INSERT INTO metadata (key, value) VALUES('version', '0');")).await.unwrap();
    metadata_file
  }

  pub async fn write(&self, entry: &FileMetadata) -> Result<i64, sqlx::Error> {
    let query = sqlx::query("INSERT INTO files (id, name, mtime, mode, ttype, destination, data_hash) VALUES(?, ?, ?, ?, ?, ?, ?);")
      .bind(entry.uid)
      .bind(entry.name.clone())
      .bind(entry.mtime)
      .bind(entry.mode)
      .bind(entry.ttype.to_string())
      .bind(entry.destination.clone())
      .bind(entry.data_hash.clone());
    let id = self.pool.execute(query).await?.last_insert_rowid();
    Ok(id)
  }

  pub async fn write_metadata(&self, key: &str, value: &str) {
    self.pool.execute(
      sqlx::query("INSERT INTO metadata (key, value) VALUES(?, ?);")
        .bind(key)
        .bind(value)
    ).await.unwrap();
  }

  pub async fn close(&self) -> () {
    return self.pool.close().await
  }
}