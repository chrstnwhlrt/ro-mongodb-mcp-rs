mod config;
mod connection;
mod direct_connection;
mod k8s_client;
mod k8s_connection;
mod mcp;
mod mongodb;
mod saved_queries;
mod tools;

use anyhow::Result;
use clap::Parser;
use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use connection::ConnectionRegistry;
use direct_connection::DirectConnection;
use k8s_connection::K8sConnection;

/// A Model Context Protocol (MCP) server for querying `MongoDB` databases.
///
/// This server enables LLMs to execute read-only `MongoDB` queries against configured connections.
/// Supports both Kubernetes namespace connections and direct MongoDB URL connections.
/// It communicates via JSON-RPC 2.0 over stdin/stdout.
#[derive(Parser)]
#[command(name = "ro-mongodb-mcp-rs")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "Read-only MCP server for MongoDB queries (Kubernetes and direct connections)", long_about = None)]
struct Cli {}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse CLI arguments (handles --version and --help automatically)
    let _cli = Cli::parse();
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ro_mongodb_mcp_rs=info".into()),
        )
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .init();

    let config = config::Config::load()?;
    tracing::info!(
        "Configuration loaded from {:?}",
        config::Config::config_file()?
    );
    tracing::info!("Data directory: {:?}", config::Config::data_dir()?);

    // Validate no duplicate connection names
    config.validate_unique_names()?;

    tracing::info!(
        "Configured connections: {} K8s namespaces, {} direct connections",
        config.namespaces.len(),
        config.connections.len()
    );

    // Build connection registry
    let mut registry = ConnectionRegistry::new();

    // Register K8s namespace connections (requires K8s client)
    if !config.namespaces.is_empty() {
        let k8s_client =
            Arc::new(k8s_client::K8sClient::new(config.kubeconfig_path.clone()).await?);
        tracing::info!("Kubernetes client initialized");

        for ns in &config.namespaces {
            tracing::info!("Registering K8s connection: {}", ns.namespace_name);
            registry.register(Box::new(K8sConnection::new(ns.clone(), k8s_client.clone())));
        }
    }

    // Register direct MongoDB connections (lazy init, no connection yet)
    for conn in &config.connections {
        tracing::info!("Registering direct connection: {}", conn.name);
        registry.register(Box::new(DirectConnection::new(conn.clone())));
    }

    let mcp_server = mcp::McpServer::new(
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        config,
        registry,
    );
    mcp_server.run().await?;

    Ok(())
}
