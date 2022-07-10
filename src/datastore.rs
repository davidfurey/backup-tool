use crate::swift;
use swift::Bucket;


pub struct DataStore {
  pub id: i32,
  pub container: String,
}

impl DataStore {
  pub fn init(&self) -> Bucket {
    let os = openstack::Cloud::from_env()
            .expect("Failed to create an identity provider from the environment");
    swift::Bucket::new(os, "bucket")
  }
}
