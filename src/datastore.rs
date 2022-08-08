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
  pub fn init(&self) -> Bucket {
    let os = openstack::Cloud::from_env()
            .expect("Failed to create an identity provider from the environment");
    swift::Bucket::new(os, &self.data_container)
  }

  pub fn metadata_bucket(&self) -> Bucket {
    let os = openstack::Cloud::from_env()
            .expect("Failed to create an identity provider from the environment");
    swift::Bucket::new(os, &&self.metadata_container)
  }
}
