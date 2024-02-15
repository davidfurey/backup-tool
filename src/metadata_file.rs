use std::path::PathBuf;
use std::fs::File;
use indicatif::{ProgressBar, ProgressStyle, MultiProgress, ProgressFinish};
use log::{trace, info};
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
pub async fn write_metadata_file(path: &PathBuf, vec: Vec<FileMetadata>, stores: Vec<DataStore>, encrypting_key: &Cert, signing_key: &Option<Cert>, mp: &MultiProgress) {
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

  let policy = openpgp::policy::StandardPolicy::new();
  let mut enc = encryption::encryptor(&policy, &mut destination, &encrypting_key, &signing_key).unwrap();
  data.serialize(&mut Serializer::new(&mut enc)).unwrap();
  enc.finalize().unwrap();

  let x = stores.get(0).unwrap().metadata_bucket().await; // todo: write to all buckets
  let metadata_file = std::fs::File::open(&filename).unwrap();

  let pb = mp.add(ProgressBar::new(metadata_file.metadata().unwrap().len()))
    .with_finish(ProgressFinish::AndLeave);
  let style =
        ProgressStyle::with_template("{prefix:.bold.dim} {spinner:.green} [{elapsed_precise}] {msg} [{wide_bar:.cyan/blue}] {bytes}/{total_bytes}")
            .unwrap()
            .progress_chars("#>-");

  pb.set_style(style.clone());
  pb.set_message(format!("{}", &name));
  pb.set_prefix("[Upload] ");
  let callback = move |bytes: usize| {
      pb.inc(u64::try_from(bytes).unwrap_or(0));
      if pb.is_finished() {
          //pb.finish_and_clear();
      }
  };

  x.upload_with_progress(&name, metadata_file, callback).await.unwrap();
  std::fs::remove_file(&filename).unwrap();

  // match bucket.upload_with_progress(&key, encrypted_file, callback).await {
  //     Ok(_) => {
  //         success_ids.push(store.id);
  //     },
  //     _ => {
  //         error!("Failed to upload {:?} to {:?}\n", request.data_hash, store.id)
  //     }
  // }

  info!("Uploaded metadata {}", &name);
  trace!("Metadata written");
}