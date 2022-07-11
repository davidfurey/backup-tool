use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::collections::HashMap;
use std::fs::File;

use serde::Serialize;
use rmps::Serializer;

use crate::FileType;
use crate::DataStore;

#[derive(Debug, Deserialize, Serialize)]
pub struct FileData {
    data: Vec<FileMetadata>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FileMetadata {
    pub name: String,
    pub mtime: i64,
    pub mode: u32,
    pub xattr: HashMap<String, String>,
    pub ttype: FileType,
    pub destination: Option<String>,
    pub data_hash: Option<String>,
}

pub fn write_metadata_file(metadata_rx: Receiver<FileMetadata>, stores: Arc<Vec<DataStore>>) {
  let mut vec = Vec::new();

  while let Ok(msg) = metadata_rx.recv() {
      vec.push(msg);
  }

  let data = FileData {
      data: vec,
  };

  let filename = "test.data";

  let destination = File::create(filename).unwrap();

  data.serialize(&mut Serializer::new(destination)).unwrap();

  for store in stores.iter() {
    let metadata_file = std::fs::File::open(filename).unwrap();
    store.metadata_bucket().upload("foo", metadata_file).unwrap();
  }
}