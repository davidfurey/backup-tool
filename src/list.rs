use std::collections::{BTreeMap, BTreeSet};
use log::error;
use crate::datastore;
use datastore::DataStore;

pub async fn list_backups(stores: &[DataStore]) {
  // Map each backup name to the set of store ids that hold it.
  let mut presence: BTreeMap<String, BTreeSet<i32>> = BTreeMap::new();
  for store in stores {
    let bucket = store.init().await;
    let mut marker: Option<String> = None;

    loop {
      let objects = match bucket
        .list(Some(store.metadata_prefix.as_str()), marker.as_deref())
        .await
      {
        Ok(objs) => objs,
        Err(e) => {
          error!("Failed to list store {}: {}", store.id, e);
          break;
        }
      };

      if objects.is_empty() {
        break;
      }

      for obj in &objects {
        if let Some(name) = obj.name
          .strip_prefix(store.metadata_prefix.as_str())
          .and_then(|n| n.strip_suffix(".metadata"))
        {
          presence.entry(name.to_string()).or_default().insert(store.id);
        }
      }

      marker = objects.last().map(|obj| obj.name.clone());
    }
  }

  let all_ids: BTreeSet<i32> = stores.iter().map(|s| s.id).collect();
  for (name, found_in) in &presence {
    if found_in == &all_ids {
      println!("{}", name);
    } else {
      let missing: Vec<i32> = all_ids.difference(found_in).copied().collect();
      println!("{} [missing from store(s): {}]", name,
        missing.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(", "));
    }
  }
}