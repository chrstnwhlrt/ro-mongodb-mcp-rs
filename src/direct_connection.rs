//! Direct MongoDB connection implementation.

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use futures::TryStreamExt;
use mongodb::{Client, bson::Document, options::ClientOptions};
use std::time::Duration;
use tokio::sync::OnceCell;

use crate::config::DirectConnectionConfig;
use crate::connection::MongoConnection;
use crate::mongodb::{QueryOperation, QueryOptions};

/// Direct MongoDB connection via URL.
/// Uses the mongodb crate to connect directly without Kubernetes.
pub struct DirectConnection {
    config: DirectConnectionConfig,
    client: OnceCell<Client>,
}

impl DirectConnection {
    pub fn new(config: DirectConnectionConfig) -> Self {
        Self {
            config,
            client: OnceCell::new(),
        }
    }

    /// Lazily initialize the MongoDB client on first use
    async fn get_client(&self) -> Result<&Client> {
        self.client
            .get_or_try_init(|| async {
                tracing::info!(
                    "Initializing direct MongoDB connection '{}'",
                    self.config.name
                );

                let mut client_options = ClientOptions::parse(&self.config.mongodb_url)
                    .await
                    .context("Failed to parse MongoDB connection URL")?;

                // Set reasonable defaults
                client_options.connect_timeout = Some(Duration::from_secs(10));
                client_options.server_selection_timeout = Some(Duration::from_secs(30));

                Client::with_options(client_options).context("Failed to create MongoDB client")
            })
            .await
    }

    async fn execute_operation(
        &self,
        collection: &mongodb::Collection<Document>,
        operation: &QueryOperation,
        query_str: &str,
        options: &QueryOptions,
    ) -> Result<String> {
        match operation {
            QueryOperation::Find => {
                let filter: Document = serde_json::from_str(query_str)
                    .context("Invalid query JSON for find operation")?;

                // Build find options
                let mut find_options = mongodb::options::FindOptions::default();

                if let Some(limit) = options.limit {
                    find_options.limit = Some(i64::from(limit));
                }

                if let Some(sort_str) = &options.sort {
                    let sort: Document =
                        serde_json::from_str(sort_str).context("Invalid sort JSON")?;
                    find_options.sort = Some(sort);
                }

                if let Some(projection_str) = &options.projection {
                    let projection: Document =
                        serde_json::from_str(projection_str).context("Invalid projection JSON")?;
                    find_options.projection = Some(projection);
                }

                let cursor = collection
                    .find(filter)
                    .with_options(find_options)
                    .await
                    .context("Find query failed")?;
                let docs: Vec<Document> = cursor
                    .try_collect()
                    .await
                    .context("Failed to collect find results")?;
                serde_json::to_string(&docs).context("Failed to serialize find results")
            }
            QueryOperation::Aggregate => {
                let pipeline: Vec<Document> =
                    serde_json::from_str(query_str).context("Invalid aggregation pipeline JSON")?;
                let cursor = collection
                    .aggregate(pipeline)
                    .await
                    .context("Aggregate query failed")?;
                let docs: Vec<Document> = cursor
                    .try_collect()
                    .await
                    .context("Failed to collect aggregate results")?;
                serde_json::to_string(&docs).context("Failed to serialize aggregate results")
            }
            QueryOperation::CountDocuments => {
                let filter: Document = serde_json::from_str(query_str)
                    .context("Invalid query JSON for countDocuments")?;
                let count = collection
                    .count_documents(filter)
                    .await
                    .context("CountDocuments query failed")?;
                Ok(count.to_string())
            }
            QueryOperation::Distinct => {
                // Get field from options or legacy format
                let field = if let Some(field) = &options.distinct_field {
                    field.clone()
                } else {
                    let params: serde_json::Value =
                        serde_json::from_str(query_str).context("Invalid distinct query JSON")?;
                    params
                        .get("field")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow!("Distinct requires 'distinct_field' parameter"))?
                        .to_string()
                };

                // Get filter from query or legacy format
                let filter: Document = if options.distinct_field.is_some() {
                    // Query is the filter directly
                    serde_json::from_str(query_str).context("Invalid filter JSON for distinct")?
                } else {
                    // Legacy format - extract from {"field": ..., "query": ...}
                    let params: serde_json::Value = serde_json::from_str(query_str)?;
                    params
                        .get("query")
                        .map(|v| serde_json::from_value(v.clone()))
                        .transpose()
                        .context("Invalid filter in distinct query")?
                        .unwrap_or_default()
                };

                let values = collection
                    .distinct(&field, filter)
                    .await
                    .context("Distinct query failed")?;

                serde_json::to_string(&values).context("Failed to serialize distinct results")
            }
        }
    }
}

#[async_trait]
impl MongoConnection for DirectConnection {
    fn name(&self) -> &str {
        &self.config.name
    }

    fn connection_type(&self) -> &str {
        "direct"
    }

    fn data_model_path(&self) -> Option<&str> {
        self.config.data_model_file_path.as_deref()
    }

    fn database_name(&self) -> &str {
        &self.config.database_name
    }

    async fn list_collections(&self) -> Result<Vec<String>> {
        let client = self.get_client().await?;
        let db = client.database(&self.config.database_name);

        let mut collections = db
            .list_collection_names()
            .await
            .context("Failed to list collections")?;

        // Sort for deterministic output
        collections.sort();

        Ok(collections)
    }

    async fn execute_query(
        &self,
        collection: &str,
        operation: &QueryOperation,
        query: &str,
        options: &QueryOptions,
        timeout_secs: u64,
    ) -> Result<String> {
        let client = self.get_client().await?;
        let db = client.database(&self.config.database_name);
        let coll = db.collection::<Document>(collection);

        tracing::info!(
            "Executing {:?} on {}.{} via direct connection '{}'",
            operation,
            self.config.database_name,
            collection,
            self.config.name
        );

        // Execute with timeout
        match tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            self.execute_operation(&coll, operation, query, options),
        )
        .await
        {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(e)) => Err(e), // Preserve original error
            Err(_) => Err(anyhow!("Query timed out after {} seconds", timeout_secs)),
        }
    }
}
