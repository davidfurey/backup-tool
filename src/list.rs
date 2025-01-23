use crate::datastore;
use datastore::DataStore;

pub async fn list_backups(store: &DataStore) {
  let metadata_bucket = store.metadata_bucket().await;
  let objects = metadata_bucket.list(Some(store.metadata_prefix.as_str()), None).await;
  objects.unwrap().iter().for_each(|obj| {
    if obj.name.ends_with(".metadata") {
      println!("{}", &obj.name[store.metadata_prefix.len()..obj.name.len() - 9]);
    }
  });
}