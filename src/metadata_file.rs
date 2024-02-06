use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::fs::File;
use log::trace;
use serde::Serialize;
extern crate rmp_serde as rmps;
use rmps::Serializer;
use crate::filetype;
use filetype::FileType;
use crate::datastore;
use datastore::DataStore;
use chrono::prelude::{Utc, SecondsFormat};
use rand::{distributions::Alphanumeric, Rng};
use crate::encryption;
extern crate sequoia_openpgp as openpgp;
use openpgp::Cert;

#[derive(Debug, Deserialize, Serialize)]
pub struct FileData {
    pub data: Vec<FileMetadata>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FileMetadata {
    pub name: String,
    pub mtime: i64,
    pub mode: u32,
    pub ttype: FileType,
    pub destination: Option<String>,
    pub data_hash: Option<String>,
}

// NB - arrays in messagepack announce their length upfront, so streaming is not possible
pub async fn write_metadata_file2(path: &PathBuf, vec: Vec<FileMetadata>, stores: Vec<DataStore>, key: &Cert) {
  let data = FileData {
      data: vec,
  };

  let random_suffix: String = rand::thread_rng()
    .sample_iter(&Alphanumeric)
    .take(4)
    .map(char::from)
    .collect();

  let datetime = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);

  let name = format!("backup-{}-{}.metadata", datetime, random_suffix);

  let filename = path.join(&name);

  trace!("Filename: {:?}", filename);
  let mut destination = File::create(&filename).unwrap();

  //todo: should be encrypted [done] (and signed?)
  let policy = openpgp::policy::StandardPolicy::new();
  let mut enc = encryption::encryptor(&policy, &mut destination, &key).unwrap();
  data.serialize(&mut Serializer::new(&mut enc)).unwrap();
  enc.finalize().unwrap();

  let x = stores.get(0).unwrap().metadata_bucket().await;
  let metadata_file = std::fs::File::open(&filename).unwrap();
  x.upload(&name, metadata_file).await.unwrap(); // todo
  trace!("Metadata written");
}