//! Saved query persistence.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::config::Config;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedQuery {
    pub name: String,
    pub description: String,
    pub collection: String,
    pub operation: String,
    pub query: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SavedQueries {
    pub queries: Vec<SavedQuery>,
}

impl SavedQueries {
    /// Get the file path for a connection's saved queries
    fn queries_file_path(connection_name: &str) -> Result<PathBuf> {
        let data_dir = Config::data_dir()?;
        Ok(data_dir.join(format!("{connection_name}.queries.yaml")))
    }

    /// Load saved queries for a connection
    pub fn load(connection_name: &str) -> Result<Self> {
        let file_path = Self::queries_file_path(connection_name)?;

        if !file_path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(&file_path)
            .context("Failed to read saved queries file")?;

        serde_yaml::from_str(&content)
            .context("Failed to parse saved queries file")
    }

    /// Save queries to file
    pub fn save(&self, connection_name: &str) -> Result<()> {
        let file_path = Self::queries_file_path(connection_name)?;
        let content = serde_yaml::to_string(self)
            .context("Failed to serialize saved queries")?;

        fs::write(&file_path, content)
            .context("Failed to write saved queries file")?;

        Ok(())
    }

    /// Add or update a query
    pub fn upsert_query(
        &mut self,
        name: String,
        description: String,
        collection: String,
        operation: String,
        query: String,
    ) {
        let now = Utc::now();

        if let Some(existing) = self.queries.iter_mut().find(|q| q.name == name) {
            // Update existing query
            existing.description = description;
            existing.collection = collection;
            existing.operation = operation;
            existing.query = query;
            existing.updated_at = now;
        } else {
            // Create new query
            self.queries.push(SavedQuery {
                name,
                description,
                collection,
                operation,
                query,
                created_at: now,
                updated_at: now,
            });
        }
    }

    /// Get a specific query by name
    pub fn get_query(&self, name: &str) -> Option<&SavedQuery> {
        self.queries.iter().find(|q| q.name == name)
    }

    /// Delete a query by name
    pub fn delete_query(&mut self, name: &str) -> bool {
        let original_len = self.queries.len();
        self.queries.retain(|q| q.name != name);
        self.queries.len() < original_len
    }

    /// List all query names
    #[allow(dead_code)]
    pub fn list_names(&self) -> Vec<String> {
        self.queries.iter().map(|q| q.name.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upsert_query() {
        let mut queries = SavedQueries::default();

        // Insert new query
        queries.upsert_query(
            "test_query".to_string(),
            "Test description".to_string(),
            "users".to_string(),
            "find".to_string(),
            "{}".to_string(),
        );

        assert_eq!(queries.queries.len(), 1);
        assert_eq!(queries.queries[0].name, "test_query");

        // Update existing query
        queries.upsert_query(
            "test_query".to_string(),
            "Updated description".to_string(),
            "users".to_string(),
            "find".to_string(),
            "{}".to_string(),
        );

        assert_eq!(queries.queries.len(), 1);
        assert_eq!(queries.queries[0].description, "Updated description");
    }

    #[test]
    fn test_get_query() {
        let mut queries = SavedQueries::default();
        queries.upsert_query(
            "test".to_string(),
            "desc".to_string(),
            "col".to_string(),
            "find".to_string(),
            "{}".to_string(),
        );

        assert!(queries.get_query("test").is_some());
        assert!(queries.get_query("nonexistent").is_none());
    }

    #[test]
    fn test_delete_query() {
        let mut queries = SavedQueries::default();
        queries.upsert_query(
            "test".to_string(),
            "desc".to_string(),
            "col".to_string(),
            "find".to_string(),
            "{}".to_string(),
        );

        assert!(queries.delete_query("test"));
        assert_eq!(queries.queries.len(), 0);
        assert!(!queries.delete_query("test"));
    }

    #[test]
    fn test_list_names() {
        let mut queries = SavedQueries::default();
        queries.upsert_query("q1".to_string(), "d1".to_string(), "c".to_string(), "find".to_string(), "{}".to_string());
        queries.upsert_query("q2".to_string(), "d2".to_string(), "c".to_string(), "find".to_string(), "{}".to_string());

        let names = queries.list_names();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"q1".to_string()));
        assert!(names.contains(&"q2".to_string()));
    }
}
