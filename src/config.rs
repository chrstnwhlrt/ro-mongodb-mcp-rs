use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Expand environment variables and tilde in a path string.
/// Supports: $HOME, ${VAR}, ~/path
fn expand_path(path: &str) -> String {
    shellexpand::full(path)
        .map(|s| s.into_owned())
        .unwrap_or_else(|_| path.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamespaceConfig {
    pub namespace_name: String,
    pub deployment_name: String,
    pub database_name: String,
    #[serde(default)]
    pub data_model_file_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectConnectionConfig {
    pub name: String,
    pub mongodb_url: String,
    pub database_name: String,
    #[serde(default)]
    pub data_model_file_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub kubeconfig_path: Option<String>,

    #[serde(default)]
    pub namespaces: Vec<NamespaceConfig>,

    #[serde(default)]
    pub connections: Vec<DirectConnectionConfig>,
}

impl Config {
    pub fn config_dir() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .context("Failed to get config directory")?
            .join("ro-mongodb-mcp-rs");

        if !config_dir.exists() {
            fs::create_dir_all(&config_dir).context("Failed to create config directory")?;
        }

        Ok(config_dir)
    }

    pub fn data_dir() -> Result<PathBuf> {
        let data_dir = dirs::data_local_dir()
            .context("Failed to get data directory")?
            .join("ro-mongodb-mcp-rs");

        if !data_dir.exists() {
            fs::create_dir_all(&data_dir).context("Failed to create data directory")?;
        }

        Ok(data_dir)
    }

    pub fn config_file() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("config.yaml"))
    }

    pub fn load() -> Result<Self> {
        let config_file = Self::config_file()?;

        if !config_file.exists() {
            Self::create_example_config(&config_file)?;
            anyhow::bail!(
                "Configuration file not found. An example configuration has been created at:\n\
                {}\n\n\
                Please edit this file to configure your MongoDB namespace environments.",
                config_file.display()
            );
        }

        let content = fs::read_to_string(&config_file).context("Failed to read config file")?;

        let mut config: Self =
            serde_yaml::from_str(&content).context("Failed to parse config file")?;

        // Expand environment variables and tilde in paths
        config.expand_paths();

        // Validate data model files exist
        config.validate();

        Ok(config)
    }

    fn create_example_config(config_file: &Path) -> Result<()> {
        let example_content = r"# ro-mongodb-mcp-rs configuration

# Optional: Path to custom kubeconfig file
# If not specified, will use default kubeconfig location
# kubeconfig_path: /path/to/custom/kubeconfig

# Kubernetes namespace connections
# Use these when MongoDB is running in a Kubernetes cluster
namespaces:
  # Example: Production environment
  - namespace_name: production
    deployment_name: mongodb
    database_name: myapp
    data_model_file_path: /path/to/data-models/production.ts  # optional

  # Example: Staging environment
  - namespace_name: staging
    deployment_name: mongodb
    database_name: myapp_staging
    # data_model_file_path is optional - omit if no schema docs available

# Direct MongoDB URL connections
# Use these for direct connections (local, Atlas, or any MongoDB with URL access)
connections:
  # Example: Local development MongoDB
  - name: local-dev
    mongodb_url: mongodb://localhost:27017
    database_name: dev_db
    # data_model_file_path: /path/to/schema.md  # optional

  # Example: MongoDB Atlas (cloud)
  # - name: atlas-analytics
  #   mongodb_url: mongodb+srv://user:password@cluster.mongodb.net
  #   database_name: analytics
  #   data_model_file_path: /path/to/data-models/analytics.md

# Configuration notes:
#
# For Kubernetes namespaces:
# - namespace_name: The Kubernetes namespace where MongoDB is deployed (also used as connection name)
# - deployment_name: The deployment label (app=<deployment_name>) to find MongoDB pods
# - database_name: The MongoDB database to query
# - data_model_file_path: (optional) Local file containing data model documentation
# - MongoDB credentials are automatically discovered from pod environment variables:
#   MONGO_INITDB_ROOT_USERNAME_FILE and MONGO_INITDB_ROOT_PASSWORD_FILE
#
# For direct connections:
# - name: Unique connection name (must not conflict with namespace names)
# - mongodb_url: Full MongoDB connection URL (credentials included)
#   WARNING: This URL may contain sensitive credentials - keep config file secure!
# - database_name: The MongoDB database to query
# - data_model_file_path: (optional) Local file containing data model documentation
";

        fs::write(config_file, example_content).context("Failed to write example config file")?;

        Ok(())
    }

    #[allow(dead_code)]
    pub fn get_namespace(&self, name: &str) -> Option<&NamespaceConfig> {
        self.namespaces.iter().find(|ns| ns.namespace_name == name)
    }

    #[allow(dead_code)]
    pub fn get_direct_connection(&self, name: &str) -> Option<&DirectConnectionConfig> {
        self.connections.iter().find(|c| c.name == name)
    }

    /// Get all connection names (both namespaces and direct connections)
    #[allow(dead_code)]
    pub fn all_connection_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .namespaces
            .iter()
            .map(|ns| ns.namespace_name.clone())
            .collect();
        names.extend(self.connections.iter().map(|c| c.name.clone()));
        names
    }

    /// Validate that no duplicate connection names exist
    pub fn validate_unique_names(&self) -> Result<()> {
        let mut seen = HashSet::new();

        for ns in &self.namespaces {
            if !seen.insert(&ns.namespace_name) {
                bail!(
                    "Duplicate connection name '{}' found in namespaces",
                    ns.namespace_name
                );
            }
        }

        for conn in &self.connections {
            if !seen.insert(&conn.name) {
                bail!(
                    "Duplicate connection name '{}' found (conflicts with namespace or another connection)",
                    conn.name
                );
            }
        }

        Ok(())
    }

    /// Expand environment variables and tilde in all path fields
    fn expand_paths(&mut self) {
        // Expand kubeconfig_path
        if let Some(path) = &self.kubeconfig_path {
            self.kubeconfig_path = Some(expand_path(path));
        }

        // Expand data_model_file_path in namespaces
        for ns in &mut self.namespaces {
            if let Some(path) = &ns.data_model_file_path {
                ns.data_model_file_path = Some(expand_path(path));
            }
        }

        // Expand data_model_file_path in direct connections
        for conn in &mut self.connections {
            if let Some(path) = &conn.data_model_file_path {
                conn.data_model_file_path = Some(expand_path(path));
            }
        }
    }

    pub fn validate(&self) {
        for ns in &self.namespaces {
            if let Some(path_str) = &ns.data_model_file_path {
                let path = PathBuf::from(path_str);
                if !path.exists() {
                    tracing::warn!(
                        "Data model file does not exist for namespace '{}': {}",
                        ns.namespace_name,
                        path_str
                    );
                }
            }
        }

        for conn in &self.connections {
            if let Some(path_str) = &conn.data_model_file_path {
                let path = PathBuf::from(path_str);
                if !path.exists() {
                    tracing::warn!(
                        "Data model file does not exist for connection '{}': {}",
                        conn.name,
                        path_str
                    );
                }
            }
        }
    }
}
