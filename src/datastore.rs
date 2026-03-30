use crate::swift;
use log::trace;
use osauth::CloudConfig;
use swift::Bucket;

fn default_true() -> bool { true }

#[derive(Deserialize)]
pub struct DataStore {
  pub id: i32,
  pub container: String,
  pub data_prefix: String,
  pub metadata_prefix: String,
  pub cloud_config: Option<CloudConfig>,
  /// Whether data objects should be uploaded to this store (default: true).
  #[serde(default = "default_true")]
  pub upload_data: bool,
  /// Whether the metadata file should be uploaded to this store (default: true).
  #[serde(default = "default_true")]
  pub upload_metadata: bool,
}

impl Clone for DataStore {
  fn clone(&self) -> DataStore {
      DataStore {
        id: self.id,
        container: self.container.clone(),
        data_prefix: self.data_prefix.clone(),
        metadata_prefix: self.metadata_prefix.clone(),
        cloud_config: self.cloud_config.clone(),
        upload_data: self.upload_data,
        upload_metadata: self.upload_metadata,
      }
  }
}

impl DataStore {
  pub async fn init(&self) -> Bucket {
    trace!("datastore::init");
    let session = match self.cloud_config.clone() {
      Some(config) => config.create_session().await,
      None =>  osauth::Session::from_env().await
    }.expect("Failed to create an identity provider");
    swift::Bucket::new(session, &self.container)
  }
}
