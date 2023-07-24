use crate::swift;
use swift::Bucket;

#[derive(Deserialize)]
pub struct DataStore {
  pub id: i32,
  pub data_container: String,
  pub metadata_container: String,
  pub data_prefix: String,
  pub metadata_prefix: String,
}

impl DataStore {
  pub async fn init(&self) -> Bucket {
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
