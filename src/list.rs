use crate::datastore;
use datastore::DataStore;

pub async fn list_backups(store: &DataStore) {
  let metadata_bucket = store.metadata_bucket().await;
  // todo: key should be prefixes with store.metadata_prefix, right?
  let objects = metadata_bucket.list(None, None).await;
  objects.unwrap().iter().for_each(|obj| {
    if obj.name.ends_with(".metadata") {
      println!("{}", &obj.name[0..obj.name.len() - 9]);
    }
  });
}