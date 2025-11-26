//! MongoDB connection abstraction.
//! Methods are called dynamically through dyn dispatch from MCP tools.

use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;

use crate::mongodb::{QueryOperation, QueryOptions};

/// Unified abstraction for MongoDB connections.
/// Both K8s namespace connections and direct URL connections implement this trait.
#[async_trait]
pub trait MongoConnection: Send + Sync {
    /// Unique identifier for this connection
    fn name(&self) -> &str;

    /// Human-readable connection type for error messages ("kubernetes" or "direct")
    fn connection_type(&self) -> &str;

    /// Path to the data model file
    fn data_model_path(&self) -> &str;

    /// Database name for this connection
    fn database_name(&self) -> &str;

    /// List all collections in the database
    async fn list_collections(&self) -> Result<Vec<String>>;

    /// Execute a MongoDB query and return the result as a string
    async fn execute_query(
        &self,
        collection: &str,
        operation: &QueryOperation,
        query: &str,
        options: &QueryOptions,
        timeout_secs: u64,
    ) -> Result<String>;
}

/// Registry holding all configured connections
#[derive(Default)]
pub struct ConnectionRegistry {
    connections: HashMap<String, Box<dyn MongoConnection>>,
}

impl ConnectionRegistry {
    pub fn new() -> Self {
        Self {
            connections: HashMap::new(),
        }
    }

    pub fn register(&mut self, conn: Box<dyn MongoConnection>) {
        let name = conn.name().to_string();
        self.connections.insert(name, conn);
    }

    pub fn get(&self, name: &str) -> Option<&dyn MongoConnection> {
        self.connections.get(name).map(|c| c.as_ref())
    }

    pub fn list_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.connections.keys().cloned().collect();
        names.sort();
        names
    }

    pub fn list_with_types(&self) -> Vec<(String, String)> {
        let mut list: Vec<_> = self
            .connections
            .iter()
            .map(|(name, conn)| (name.clone(), conn.connection_type().to_string()))
            .collect();
        list.sort_by(|a, b| a.0.cmp(&b.0));
        list
    }
}

