use crate::swift;
use log::trace;
use osauth::CloudConfig;
use swift::Bucket;

#[derive(Deserialize)]
pub struct DataStore {
  pub id: i32,
  pub data_container: String,
  pub metadata_container: String,
  pub data_prefix: String,
  pub metadata_prefix: String,
  pub metadata_cloud_config: Option<CloudConfig>,
  pub data_cloud_config: Option<CloudConfig>
}

impl Clone for DataStore {
  fn clone(&self) -> DataStore {
      DataStore {
        id: self.id,
        data_container: self.data_container.clone(),
        metadata_container: self.metadata_container.clone(),
        data_prefix: self.data_prefix.clone(),
        metadata_prefix: self.metadata_prefix.clone(),
        metadata_cloud_config: self.metadata_cloud_config.clone(),
        data_cloud_config: self.data_cloud_config.clone()
      }
  }
}

impl DataStore {
  pub async fn init(&self) -> Bucket {
    trace!("datastore::init");
    let session = match self.data_cloud_config.clone() {
      Some(config) => config.create_session().await,
      None =>  osauth::Session::from_env().await
    }.expect("Failed to create an identity provider");
    swift::Bucket::new(session, &self.data_container)
  }

  pub async fn metadata_bucket(&self) -> Bucket {
    let session = match self.metadata_cloud_config.clone() {
      Some(config) => config.create_session().await,
      None =>  osauth::Session::from_env().await
    }.expect("Failed to create an identity provider");
    swift::Bucket::new(session, &self.metadata_container)
  }
}
