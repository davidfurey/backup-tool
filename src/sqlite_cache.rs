use std::fs::Metadata;
use std::path::Path;

use crate::datastore;
use crate::hash;

use std::os::unix::prelude::MetadataExt;
use datastore::DataStore;
use sqlx::Executor;
use sqlx::Row;
use sqlx::SqlitePool;
use sqlx;

pub struct AsyncCache {
  pool: SqlitePool
}

impl Clone for AsyncCache {
  fn clone(&self) -> AsyncCache {
      AsyncCache {
        pool: self.pool.clone(),
      }
  }
}

use std::str::FromStr;

impl AsyncCache {
  pub async fn new<'b>() -> AsyncCache {
    let options = sqlx::sqlite::SqliteConnectOptions::from_str("sqlite:cache.db?mode=rwc").unwrap()
      .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
    AsyncCache {
      pool: SqlitePool::connect_with(options).await.unwrap()
    }
  }

  pub async fn set_data_in_cold_storage(&self, hash: &str, md5_hash: &str, store_ids: &Vec<i32>) -> Result<usize, String> {
    for store_id in store_ids {
      let query =
        sqlx::query("INSERT INTO uploaded_objects VALUES ($1, $2, $3)")
          .bind(hash)
          .bind(md5_hash)
          .bind(store_id);
      if self.pool.execute(query).await.unwrap().rows_affected() != 1 {
        return Err("Query failed".to_string())
      }
    }
    return Ok(1);
  }

  pub async fn lock_data(&self, hash: &str) -> bool {
    let query =
      sqlx::query("INSERT INTO hash_lock VALUES ($1)")
        .bind(hash);
    match self.pool.execute(query).await {
      Ok(_) => true,
      Err(_) => false
    }
  }

  pub async fn is_data_in_cold_storage(&self, data_hash: &String, stores: &Vec<DataStore>) -> Result<bool, sqlx::Error> {
    for store in stores {
      let store_id = store.id.to_string();
      let query = sqlx::query("SELECT 1 FROM uploaded_objects WHERE data_hash = ? and datastore_id = ?")
        .bind(data_hash)
        .bind(store_id.as_str());
      let row = self.pool.fetch_one(query).await;

      match row {
          Ok(_) => { },
          Err(sqlx::Error::RowNotFound) => return Ok(false),
          Err(x) => return Err(x)
      };
    }
    return Ok(true);
  }

  pub async fn init(&self) {
    self.pool.execute(sqlx::query("CREATE TABLE IF NOT EXISTS fs_hash_cache (fs_hash CHARACTER(128) UNIQUE, data_hash CHARACTER(128) NULL, in_use BOOLEAN);")).await.unwrap();
    self.pool.execute(sqlx::query("CREATE TABLE IF NOT EXISTS uploaded_objects (data_hash TEXT, encrypted_md5 TEXT NULL, datastore_id INTEGER, UNIQUE(data_hash, datastore_id));")).await.unwrap();
    self.pool.execute(sqlx::query("CREATE TABLE IF NOT EXISTS hash_lock (data_hash TEXT, UNIQUE(data_hash));")).await.unwrap();
    self.pool.execute(sqlx::query("UPDATE fs_hash_cache set in_use = false;")).await.unwrap();
    self.pool.execute(sqlx::query("DELETE FROM hash_lock;")).await.unwrap();
  }

  pub async fn cleanup(&self) {
    self.pool.execute(sqlx::query("DELETE FROM fs_hash_cache WHERE in_use = false;")).await.unwrap();
  }

  pub async fn try_get_hash(&self, path: &Path, metadata: &Metadata) -> Result<Option<String>, sqlx::Error> {
    use futures::TryFutureExt;
    let metadata_hash = hash::metadata(metadata.len(), metadata.mtime(), path);

    self.mark_used_and_lookup_hash(&metadata_hash)
      .and_then(|v: Option<_>| async {
        match v {
          Some(s) => return Ok(Some(s)),
          None => Ok(None)
        }
      }).await
  }

  pub async fn get_hash(&self, path: &Path, metadata: &Metadata, hmac_secret: &str) -> Result<String, sqlx::Error> {
    use futures::TryFutureExt;
    let metadata_hash = hash::metadata(metadata.len(), metadata.mtime(), path);

    self.mark_used_and_lookup_hash(&metadata_hash)
      .and_then(|v| async {
        match v {
          Some(s) => return Ok(s),
          None => {
              let data_hash = hash::data(path, hmac_secret);
              self.set_data_hash(&metadata_hash, &data_hash).await.unwrap();
              Ok(data_hash)
          }
        }
      }).await
  }

  async fn mark_used_and_lookup_hash(&self, filename: &str) -> Result<Option<String>, sqlx::Error> {
    let query = sqlx::query("INSERT INTO fs_hash_cache (fs_hash, in_use) VALUES(?, true) ON CONFLICT(fs_hash) do UPDATE set in_use = true RETURNING data_hash")
      .bind(filename);
    let row = self.pool.fetch_one(query).await;

    match row.map(|r| {
      r.try_get("data_hash").unwrap()
    }) {
        Ok(r) => Ok(r),
        Err(_) => Ok(None),
    }
  }

  pub async fn set_data_hash(&self, metadata_hash: &str, data_hash: &str) -> Result<u64, sqlx::Error> {
    let query = sqlx::query("UPDATE fs_hash_cache set data_hash = ? where fs_hash = ?").bind(data_hash).bind(metadata_hash);
    let result = self.pool.execute(query).await;
    return result.map(|x| x.rows_affected());
  }  

}