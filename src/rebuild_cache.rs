use crate::{config::BackupConfig, sqlite_cache::AsyncCache};
use log::info;

pub async fn rebuild_cache(config: BackupConfig) {
  let cache = AsyncCache::new().await;
  info!("Clearing cache");
  cache.clear_cold_storage_cache().await.unwrap();
  for store in config.stores {
    let bucket = store.init().await;
    let prefix_len = store.data_prefix.as_str().len();
    let mut count = 0;
    let mut no_more = false;
    let mut marker: Option<String> = None;
    while !no_more {
      let objects = bucket.list(Some(store.data_prefix.as_str()), marker.as_deref()).await.unwrap();
      count += objects.len();
      no_more = objects.is_empty();
      marker = objects.last().map(|m| m.name.to_owned());
      for object in objects {
        cache.set_data_in_cold_storage(&object.name[prefix_len..], &object.hash, &vec![store.id]).await.unwrap();
      }
    }
    info!("Added {} files from store {}", count, store.id);
  }
}