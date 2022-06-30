use openstack::Cloud;
use openstack::object_storage::Object;
use std::fs::File;
use openstack::Result;

//#[derive(Debug, Clone)]
pub struct Bucket<'a> {
  cloud: &'a Cloud,
  container: &'a str,
}

impl<'a> Bucket<'a> {
    //fn longest<'a>(x: &'a str, y: &'a str) -> &'a str {
    pub fn new<'b>(cloud: &'b Cloud, container: &'b str) -> Bucket<'b> {
        Bucket {
            cloud,
            container,
        }
    }

    pub fn upload(&self, key: &str, source: File) -> Result<Object> {
      return self.cloud.create_object(self.container, key, source);
    }
    //os.create_object(container, key, source);
    // pub fn endpoint_filters_mut(&mut self) -> &mut EndpointFilters {
    //     Rc::make_mut(&mut self.session).endpoint_filters_mut()
    // }
}
