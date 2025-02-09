use std::{os::unix::prelude::MetadataExt, fs::Metadata};
use crate::{metadata_file::{self, FileMetadata}, upload_worker, datastore, sqlite_cache::AsyncCache, filetype, hash};
use datastore::DataStore;
use indicatif::{MultiProgress, ProgressStyle, ProgressBar};
use log::trace;
use log::warn;
use filetype::FileType;
use upload_worker::UploadRequest;

async fn generate_hash(dir_entry: &walkdir::DirEntry, cache: &AsyncCache, hmac_secret: &String, mp: &MultiProgress, metadata: &Metadata) -> String {
    let hms = hmac_secret.clone();
    let de = dir_entry.path().to_owned().clone();
    let (send, recv) = tokio::sync::oneshot::channel();
    let spinner_style = ProgressStyle::with_template("{prefix:.bold.dim} {spinner} {wide_msg}")
        .unwrap()
        .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ");
    let mp = mp.clone();

    let filename = format!("{:?}", dir_entry.file_name());
    rayon::spawn(move || {
        let pb = mp.add(ProgressBar::new_spinner());
        pb.set_style(spinner_style.clone());
        pb.set_prefix(format!("[Hash]"));
        pb.inc(1);
        pb.set_message(format!("{}", filename));
        let res = hash::data(&de, &hms);
        pb.finish_and_clear();
        let _ = send.send(res);
    });
    let res = recv.await.expect("Panic in rayon::spawn");

    let metadata_hash = hash::metadata(metadata.len(), metadata.mtime(), dir_entry.path());
    cache.set_data_hash(&metadata_hash, &res).await.unwrap();
    res
}

pub async fn hash_work(dir_entry: walkdir::DirEntry, id: usize, cache: &AsyncCache, stores: &Vec<DataStore>, hmac_secret: &String, mp: &MultiProgress, force_hash: bool) -> (Option<UploadRequest>, Option<FileMetadata>, bool, u64) {
    let file_type: Option<FileType> = FileType::from(dir_entry.file_type());
    let mut destination: Option<String> = None;
    let mut data_hash: Option<String> = None;
    let metadata = dir_entry.metadata().unwrap();
    let mut upload_request: Option<UploadRequest> = None;
    let mut hash_cached = false;
    match file_type {
        Some(FileType::FILE) => {
            let cached_d_hash = cache.try_get_hash(dir_entry.path(), &metadata).await.unwrap();
            let d_hash = match cached_d_hash {
                Some(h) => {
                    hash_cached = true;
                    if force_hash {
                        let generated_hash = generate_hash(&dir_entry, cache, hmac_secret, mp, &metadata).await;
                        if generated_hash != h {
                            warn!("Hash in cache does not match expected value for {:?}. Updated DB to match filesystem", dir_entry.file_name());
                        }
                        generated_hash
                    } else {
                        h
                    }
                },
                None => {
                    generate_hash(&dir_entry, cache, hmac_secret, mp, &metadata).await
                }
            };
            
            let requires_upload = cache.requires_upload(&d_hash, &stores).await.unwrap();
            if !requires_upload.is_empty() {
                trace!("Sending {:?} to upload queue\n", dir_entry.file_name());
                upload_request = Some(UploadRequest {
                    filename: dir_entry.path().to_path_buf(),  
                    data_hash: d_hash.clone(),
                });
            } else {
                trace!("Skipping {:?} ({:?} already uploaded)\n", dir_entry.file_name(), d_hash);
            }
            data_hash = Some(d_hash);
        }
        Some(FileType::SYMLINK) => {
            destination = Some(std::fs::read_link(dir_entry.path()).unwrap().to_string_lossy().to_string());
            trace!("Symbolic link from {:?} to {:?}", dir_entry.path().to_string_lossy().to_string(), destination);
        }
        _ => {}
    }

    let file_metadata = file_type.map(|ttype| {
        metadata_file::FileMetadata {
            uid: id as i64,
            name: dir_entry.path().to_string_lossy().to_string(),
            mtime: metadata.mtime(),
            mode: metadata.mode(),
            ttype: ttype,
            destination,
            data_hash,
        }
    });
    (upload_request, file_metadata, hash_cached, metadata.len())
}
