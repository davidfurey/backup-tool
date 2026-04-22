use crate::{config::BackupConfig, sqlite_cache::AsyncCache};
use log::info;

pub async fn rebuild_cache(config: BackupConfig, limit: &[i32]) {
  let stores: Vec<_> = if limit.is_empty() {
    config.stores
  } else {
    config.stores.into_iter().filter(|s| limit.contains(&s.id)).collect()
  };

  let store_ids: Vec<i32> = stores.iter().map(|s| s.id).collect();

  let cache = AsyncCache::new().await;
  info!("Clearing cache for store(s): {:?}", store_ids);
  cache.clear_cold_storage_cache(&store_ids).await.unwrap();
  for store in stores {
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