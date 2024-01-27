use crate::swift;
use log::trace;
use swift::Bucket;

#[derive(Deserialize)]
pub struct DataStore {
  pub id: i32,
  pub data_container: String,
  pub metadata_container: String,
  pub data_prefix: String,
  pub metadata_prefix: String,
}

impl Clone for DataStore {
  fn clone(&self) -> DataStore {
      DataStore {
        id: self.id,
        data_container: self.data_container.clone(),
        metadata_container: self.metadata_container.clone(),
        data_prefix: self.data_prefix.clone(),
        metadata_prefix: self.metadata_prefix.clone(),
      }
  }
}

impl DataStore {
  pub async fn init(&self) -> Bucket {
    trace!("datastore::init");
    let session = osauth::Session::from_env()
      .await
      .expect("Failed to create an identity provider from the environment");
    swift::Bucket::new(session, &self.data_container)
  }

  pub async fn metadata_bucket(&self) -> Bucket {
    let session = osauth::Session::from_env()
      .await
      .expect("Failed to create an identity provider from the environment");
    swift::Bucket::new(session, &self.metadata_container)
  }
}
