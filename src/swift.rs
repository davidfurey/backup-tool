use std::fs::File;
use futures::TryStreamExt;
use log::error;
use reqwest::Method;
use osauth::Session;
use osauth::services::OBJECT_STORAGE;
use futures::stream::StreamExt;
use tokio_util::io::StreamReader;
use crate::query;
use query::Query;
use crate::bucket::ObjectEntry;

/// The Swift-backed implementation.  Use [`Bucket`] in calling code.
pub struct SwiftBucket {
  session: Session,
  container: String,
}

impl SwiftBucket {
    pub fn new(session: Session, container: &str) -> SwiftBucket {
        SwiftBucket {
            session,
            container: container.to_string(),
        }
    }

    pub async fn upload_with_progress(&self, key: &str, source: File, callback: impl Fn(usize) + Sync + Send + 'static) -> Result<(), String> {
      let tokio_file = tokio::fs::File::from(source);
      let stream = tokio_util::io::ReaderStream::new(tokio_file).inspect_ok(move |bytes| {
        callback(bytes.len())
      });
      self.session.put(OBJECT_STORAGE, &[self.container.as_ref(), key])
        .body(reqwest::Body::wrap_stream(stream))
        .send().await
        .map(|_| ())
        .map_err(|e| format!("Swift upload error: {:?}", e))
    }

    /// Returns `Ok(true)` if the object exists (2xx), `Ok(false)` if it is
    /// definitively absent (HTTP 404), or `Err` for any other non-success
    /// status or request-level failure.
    pub async fn exists(&self, key: &str) -> Result<bool, String> {
        match self.session.request(OBJECT_STORAGE, Method::HEAD, &[self.container.as_ref(), key])
            .send().await
        {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    Ok(true)
                } else if status == reqwest::StatusCode::NOT_FOUND {
                    Ok(false)
                } else {
                    let msg = format!(
                        "Unexpected status {} checking existence of {}/{}",
                        status, self.container, key
                    );
                    error!("{}", msg);
                    Err(msg)
                }
            }
            Err(e) => {
                let msg = format!("Error checking existence of {}/{}: {:?}", self.container, key, e);
                error!("{}", msg);
                Err(msg)
            }
        }
    }

    pub async fn download_with_progress(&self, key: &str, dest: File, callback: impl Fn(usize) + Sync + Send + 'static) -> std::io::Result<u64> {
      let response = self.session.get(OBJECT_STORAGE, &[self.container.as_ref(), key]).send().await.unwrap();
      let stream = response
        .bytes_stream()
        .map(move |result| {
            result.map(|bytes| {
                callback(bytes.len());
                bytes
            }).map_err(|_error| {
                error!("Encountered error");
                std::io::Error::new(std::io::ErrorKind::Other, "Error!")
            })
        });
      let mut reader = StreamReader::new(stream);
      let mut tokio_file = tokio::fs::File::from(dest);
      tokio::io::copy(&mut reader, &mut tokio_file).await
    }

    pub async fn download(&self, key: &str, dest: File) -> std::io::Result<u64> {
      let response = self.session.get(OBJECT_STORAGE, &[self.container.as_ref(), key]).send().await.unwrap();
      let stream = response
        .bytes_stream()
        .map(|result| {
            result.map_err(|_error| {
                error!("Encountered error");
                std::io::Error::new(std::io::ErrorKind::Other, "Error!")
              }
            )
        });
      let mut reader = StreamReader::new(stream);
      let mut tokio_file = tokio::fs::File::from(dest);
      tokio::io::copy(&mut reader, &mut tokio_file).await
    }

    pub async fn list(&self, prefix: Option<&str>, marker: Option<&str>) -> std::io::Result<Vec<ObjectEntry>> {
      let mut query = Query::new();
      query.push_str("format", "json");
      query.push_str("limit", "100");
      match prefix {
        Some(p) => {
          query.push_str("prefix", p);
        },
        _ => {}
      };
      match marker {
        Some(m) => {
          query.push_str("marker", m);
        },
        _ => {}
      };

      let response = self.session.get(OBJECT_STORAGE, &[self.container.as_ref(), ""])
        .query(&query)
        .send().await.unwrap()
        .json().await.unwrap();
      Ok(response)
    }
}
