//! Kubernetes-based MongoDB connection implementation.

use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use crate::config::NamespaceConfig;
use crate::connection::MongoConnection;
use crate::k8s_client::K8sClient;
use crate::mongodb::{self, MongoCredentials, MongoQuery, QueryOperation, QueryOptions};

/// TTL for cached pod info (pod name + credentials)
const CACHE_TTL: Duration = Duration::from_secs(300);

struct CachedPodInfo {
    pod_name: String,
    credentials: MongoCredentials,
    cached_at: Instant,
}

/// Kubernetes-based MongoDB connection.
/// Executes queries by running mongosh inside a MongoDB pod.
/// Caches pod discovery and credentials to avoid redundant K8s API calls.
pub struct K8sConnection {
    config: NamespaceConfig,
    k8s_client: Arc<K8sClient>,
    pod_cache: RwLock<Option<CachedPodInfo>>,
}

impl K8sConnection {
    pub fn new(config: NamespaceConfig, k8s_client: Arc<K8sClient>) -> Self {
        Self {
            config,
            k8s_client,
            pod_cache: RwLock::new(None),
        }
    }

    /// Get pod name and credentials, using cache when available.
    async fn get_pod_info(&self) -> Result<(String, MongoCredentials)> {
        // Check cache first
        {
            let cache = self.pod_cache.read().await;
            if let Some(ref cached) = *cache
                && cached.cached_at.elapsed() < CACHE_TTL
            {
                tracing::debug!(
                    "Using cached pod info for connection '{}'",
                    self.config.namespace_name
                );
                return Ok((cached.pod_name.clone(), cached.credentials.clone()));
            }
        }

        // Cache miss or expired — discover pod and credentials
        let pod_name = self
            .k8s_client
            .find_healthy_pod(&self.config.namespace_name, &self.config.deployment_name)
            .await?;

        let container_name = &self.config.deployment_name;

        let credentials = mongodb::get_mongodb_credentials(
            &self.k8s_client,
            &self.config.namespace_name,
            &pod_name,
            container_name,
        )
        .await?;

        // Update cache
        {
            let mut cache = self.pod_cache.write().await;
            *cache = Some(CachedPodInfo {
                pod_name: pod_name.clone(),
                credentials: credentials.clone(),
                cached_at: Instant::now(),
            });
        }

        tracing::info!(
            "Cached pod info for connection '{}': pod={}",
            self.config.namespace_name,
            pod_name
        );

        Ok((pod_name, credentials))
    }
}

#[async_trait]
impl MongoConnection for K8sConnection {
    fn name(&self) -> &str {
        &self.config.namespace_name
    }

    fn connection_type(&self) -> &str {
        "kubernetes"
    }

    fn data_model_path(&self) -> Option<&str> {
        self.config.data_model_file_path.as_deref()
    }

    fn database_name(&self) -> &str {
        &self.config.database_name
    }

    async fn list_collections(&self) -> Result<Vec<String>> {
        let (pod_name, credentials) = self.get_pod_info().await?;
        let container_name = &self.config.deployment_name;

        mongodb::list_collections(
            &self.k8s_client,
            &self.config.namespace_name,
            &pod_name,
            container_name,
            &credentials,
            &self.config.database_name,
        )
        .await
    }

    async fn execute_query(
        &self,
        collection: &str,
        operation: &QueryOperation,
        query: &str,
        options: &QueryOptions,
        timeout_secs: u64,
    ) -> Result<String> {
        let (pod_name, credentials) = self.get_pod_info().await?;
        let container_name = &self.config.deployment_name;

        tracing::info!(
            "Using pod: {} container: {} for connection '{}'",
            pod_name,
            container_name,
            self.config.namespace_name
        );

        let mongo_query = MongoQuery {
            database: self.config.database_name.clone(),
            collection: collection.to_string(),
            operation: operation.clone(),
            query: query.to_string(),
            options: options.clone(),
        };

        mongodb::execute_mongosh_query(
            &self.k8s_client,
            &self.config.namespace_name,
            &pod_name,
            container_name,
            &credentials,
            &mongo_query,
            timeout_secs,
        )
        .await
    }
}
