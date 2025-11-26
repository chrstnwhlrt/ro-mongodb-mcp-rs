//! Kubernetes-based MongoDB connection implementation.

use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

use crate::config::NamespaceConfig;
use crate::connection::MongoConnection;
use crate::k8s_client::K8sClient;
use crate::mongodb::{self, MongoQuery, QueryOperation, QueryOptions};

/// Kubernetes-based MongoDB connection.
/// Executes queries by running mongosh inside a MongoDB pod.
pub struct K8sConnection {
    config: NamespaceConfig,
    k8s_client: Arc<K8sClient>,
}

impl K8sConnection {
    pub fn new(config: NamespaceConfig, k8s_client: Arc<K8sClient>) -> Self {
        Self { config, k8s_client }
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

    fn data_model_path(&self) -> &str {
        &self.config.data_model_file_path
    }

    fn database_name(&self) -> &str {
        &self.config.database_name
    }

    async fn list_collections(&self) -> Result<Vec<String>> {
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
        let pod_name = self
            .k8s_client
            .find_healthy_pod(&self.config.namespace_name, &self.config.deployment_name)
            .await?;

        let container_name = &self.config.deployment_name;

        tracing::info!(
            "Using pod: {} container: {} for connection '{}'",
            pod_name,
            container_name,
            self.config.namespace_name
        );

        let credentials = mongodb::get_mongodb_credentials(
            &self.k8s_client,
            &self.config.namespace_name,
            &pod_name,
            container_name,
        )
        .await?;

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
