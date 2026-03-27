#![allow(dead_code)] // various things are unused with --no-default-features

use std::fmt;

use serde::{Serialize, Serializer};

/// Type of query parameters.
#[derive(Clone)]
pub struct Query(pub Vec<(String, String)>);

impl fmt::Debug for Query {
    fn fmt(&self, f: &mut fmt::Formatter) -> ::std::result::Result<(), fmt::Error> {
        write!(f, "{:?}", self.0)
    }
}

impl Query {
    /// Empty query.
    pub fn new() -> Query {
        Query(Vec::new())
    }

    /// Add an item to the query.
    #[allow(clippy::needless_pass_by_value)] // TODO: fix
    pub fn push<K, V>(&mut self, param: K, value: V)
    where
        K: Into<String>,
        V: ToString,
    {
        self.0.push((param.into(), value.to_string()))
    }

    /// Add a string item to the query.
    pub fn push_str<K, V>(&mut self, param: K, value: V)
    where
        K: Into<String>,
        V: Into<String>,
    {
        self.0.push((param.into(), value.into()))
    }

    /// Add marker and limit to the query and clone it.
    pub fn with_marker_and_limit(&self, limit: Option<usize>, marker: Option<String>) -> Query {
        let mut new = self.clone();
        if let Some(limit_) = limit {
            new.push("limit", limit_);
        }
        if let Some(marker_) = marker {
            new.push_str("marker", marker_);
        }
        new
    }
}

impl Serialize for Query {
    fn serialize<S>(&self, serializer: S) -> ::std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}
