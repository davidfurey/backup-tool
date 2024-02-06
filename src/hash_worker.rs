use std::os::unix::prelude::MetadataExt;
use crate::{metadata_file::{self, FileMetadata}, upload_worker, datastore, sqlite_cache::AsyncCache, filetype, hash};
use datastore::DataStore;
use indicatif::{MultiProgress, ProgressStyle, ProgressBar};
use log::trace;
use filetype::FileType;
use upload_worker::UploadRequest;


pub async fn hash_work(dir_entry: walkdir::DirEntry, cache: &AsyncCache, stores: &Vec<DataStore>, hmac_secret: &String, mp: &MultiProgress) -> (Option<UploadRequest>, Option<FileMetadata>) {
    let file_type: Option<FileType> = FileType::from(dir_entry.file_type());
    let mut destination: Option<String> = None;
    let mut data_hash: Option<String> = None;
    let metadata = dir_entry.metadata().unwrap();
    let mut upload_request: Option<UploadRequest> = None;
    match file_type {
        Some(FileType::FILE) => {
            let cached_d_hash = cache.try_get_hash(dir_entry.path(), &metadata).await.unwrap();
            let d_hash = match cached_d_hash {
                Some(h) => h,
                None => {
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
            };
            
            if !cache.is_data_in_cold_storage(&d_hash, &stores).await.unwrap() {
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
            name: dir_entry.path().to_string_lossy().to_string(),
            mtime: metadata.mtime(),
            mode: metadata.mode(),
            ttype: ttype,
            destination,
            data_hash,
        }
    });
    (upload_request, file_metadata)
}
