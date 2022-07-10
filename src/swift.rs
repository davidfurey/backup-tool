use openstack::Cloud;
use openstack::object_storage::Object;
use std::fs::File;
use openstack::Result;

//#[derive(Debug, Clone)]
pub struct Bucket<'a> {
  cloud: Cloud,
  container: &'a str,
}

impl<'a> Bucket<'a> {
    pub fn new<'b>(cloud: Cloud, container: &'b str) -> Bucket<'b> {
        Bucket {
            cloud,
            container,
        }
    }

    pub fn upload(&self, key: &str, source: File) -> Result<Object> {
      return self.cloud.create_object(self.container, key, source);
    }
}
