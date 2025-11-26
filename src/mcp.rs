//! MCP server implementation with tool handlers.

use anyhow::Result;
use rmcp::{
    ServerHandler,
    handler::server::{tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::config::Config;
use crate::connection::ConnectionRegistry;
use crate::mongodb::{self, QueryOptions};
use crate::saved_queries::SavedQueries;
use crate::tools::*;

/// Format anyhow error with full cause chain
fn format_error(e: &anyhow::Error) -> String {
    let mut msg = e.to_string();
    for cause in e.chain().skip(1) {
        msg.push_str(": ");
        msg.push_str(&cause.to_string());
    }
    msg
}

/// Find all {{placeholder}} patterns in a query string
fn find_placeholders(query: &str) -> HashSet<String> {
    let mut placeholders = HashSet::new();
    let bytes = query.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        // Look for {{
        if i + 1 < bytes.len() && bytes[i] == b'{' && bytes[i + 1] == b'{' {
            i += 2; // skip {{
            let start = i;
            // Find }}
            while i + 1 < bytes.len() {
                if bytes[i] == b'}' && bytes[i + 1] == b'}' {
                    let name = &query[start..i];
                    if !name.is_empty() {
                        placeholders.insert(name.to_string());
                    }
                    i += 2; // skip }}
                    break;
                }
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    placeholders
}

/// Substitute {{placeholder}} patterns with provided values
fn substitute_placeholders(
    query: &str,
    variables: &HashMap<String, String>,
) -> std::result::Result<String, Vec<String>> {
    let placeholders = find_placeholders(query);

    // Check for missing variables
    let missing: Vec<String> = placeholders
        .iter()
        .filter(|p| !variables.contains_key(*p))
        .cloned()
        .collect();

    if !missing.is_empty() {
        return Err(missing);
    }

    // Perform substitution
    let mut result = query.to_string();
    for (name, value) in variables {
        let pattern = format!("{{{{{}}}}}", name);
        result = result.replace(&pattern, value);
    }

    Ok(result)
}

pub struct McpServer {
    name: String,
    version: String,
    #[allow(dead_code)]
    config: Config,
    connections: Arc<ConnectionRegistry>,
    tool_router: ToolRouter<Self>,
}

impl McpServer {
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        config: Config,
        connections: ConnectionRegistry,
    ) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            config,
            connections: Arc::new(connections),
            tool_router: Self::tool_router(),
        }
    }

    pub async fn run(self) -> Result<()> {
        use rmcp::ServiceExt;

        tracing::info!("MCP server starting: {} v{}", self.name, self.version);

        let transport = rmcp::transport::stdio();
        let server = self.serve(transport).await?;
        server.waiting().await?;

        tracing::info!("MCP server shutting down");
        Ok(())
    }

    fn connection_not_found(&self, name: &str) -> rmcp::ErrorData {
        let available = self.connections.list_names().join(", ");
        rmcp::ErrorData::invalid_params(
            format!("Connection '{name}' not found. Available: {available}"),
            None,
        )
    }
}

#[tool_router]
impl McpServer {
    /// Returns the current date and time on the server.
    ///
    /// Use this when constructing time-based MongoDB queries (e.g., records from last 24 hours).
    ///
    /// Returns timestamps in both UTC and local timezone formats.
    #[tool]
    fn get_current_time(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        use chrono::{Local, Utc};

        let now_utc = Utc::now();
        let now_local = Local::now();

        let response = serde_json::json!({
            "utc": {
                "iso8601": now_utc.to_rfc3339(),
                "timestamp": now_utc.timestamp(),
                "human_readable": now_utc.format("%Y-%m-%d %H:%M:%S UTC").to_string()
            },
            "local": {
                "iso8601": now_local.to_rfc3339(),
                "timestamp": now_local.timestamp(),
                "human_readable": now_local.format("%Y-%m-%d %H:%M:%S %Z").to_string(),
                "timezone": now_local.format("%Z").to_string(),
                "offset": now_local.format("%z").to_string()
            }
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&response).unwrap(),
        )]))
    }

    /// Lists all available MongoDB connections that you can query.
    ///
    /// This includes both Kubernetes namespace connections and direct MongoDB URL connections.
    /// Use this first to discover available connections before querying.
    #[tool]
    fn list_connections(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        let connections = self.connections.list_with_types();

        let response = serde_json::json!({
            "connections": connections.iter().map(|(name, conn_type)| {
                serde_json::json!({ "name": name, "type": conn_type })
            }).collect::<Vec<_>>(),
            "count": connections.len()
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&response).unwrap(),
        )]))
    }

    /// Retrieves the data model documentation for a specific connection.
    ///
    /// Returns the contents of data_model_file_path from config (any format: markdown, TypeScript, etc.).
    /// Describes collections, fields, and data types. Read before querying.
    #[tool]
    fn get_data_model(
        &self,
        Parameters(params): Parameters<GetDataModelParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let connection = self
            .connections
            .get(&params.connection_name)
            .ok_or_else(|| self.connection_not_found(&params.connection_name))?;

        let Some(path) = connection.data_model_path() else {
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "No data model configured for connection '{}'. \
                 Add data_model_file_path to config.yaml to provide schema documentation.",
                params.connection_name
            ))]));
        };

        let content = std::fs::read_to_string(path).map_err(|e| {
            rmcp::ErrorData::internal_error(
                format!("Failed to read data model file '{}': {}", path, e),
                None,
            )
        })?;

        if content.trim().is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "Data model file is empty for connection '{}'. \
                 Add schema documentation to: {}",
                params.connection_name, path
            ))]));
        }

        Ok(CallToolResult::success(vec![Content::text(content)]))
    }

    /// Lists all MongoDB collection names in a connection.
    ///
    /// IMPORTANT: Collection names are CASE-SENSITIVE in MongoDB!
    /// Always use this tool to get exact collection names before querying.
    #[tool]
    async fn list_collections(
        &self,
        Parameters(params): Parameters<ListCollectionsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let connection = self
            .connections
            .get(&params.connection_name)
            .ok_or_else(|| self.connection_not_found(&params.connection_name))?;

        let collections = connection
            .list_collections()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format_error(&e), None))?;

        let response = serde_json::json!({
            "database": connection.database_name(),
            "collections": collections,
            "count": collections.len()
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&response).unwrap(),
        )]))
    }

    /// Executes a READ-ONLY MongoDB query against a specific collection.
    ///
    /// Operations:
    /// - find: query={"status": "active"}, limit=10, sort={"createdAt": -1}
    /// - aggregate: query=[{"$match": {}}, {"$group": {"_id": "$status"}}]
    /// - countDocuments: query={"status": "active"}
    /// - distinct: distinct_field="country", query={"active": true} ← query is filter
    ///
    /// 30-second timeout. Limit/sort/projection only apply to find.
    #[tool]
    async fn query_mongodb(
        &self,
        Parameters(params): Parameters<QueryMongodbParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let connection = self
            .connections
            .get(&params.connection_name)
            .ok_or_else(|| self.connection_not_found(&params.connection_name))?;

        let op = mongodb::QueryOperation::from_str(params.operation.as_str())
            .map_err(|e| rmcp::ErrorData::invalid_params(e.to_string(), None))?;

        // Check for overrides on non-find operations
        let has_overrides =
            params.limit.is_some() || params.sort.is_some() || params.projection.is_some();
        let is_find = matches!(op, mongodb::QueryOperation::Find);
        let warning = if has_overrides && !is_find {
            Some("Note: limit/sort/projection only apply to find operations (ignored)")
        } else {
            None
        };

        let options = QueryOptions {
            limit: params.limit,
            sort: params.sort,
            projection: params.projection,
            distinct_field: params.distinct_field,
        };

        let result = connection
            .execute_query(&params.collection_name, &op, &params.query, &options, 30)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format_error(&e), None))?;

        // Include warning if applicable
        let output = if let Some(warn) = warning {
            format!("{}\n\n{}", warn, result)
        } else {
            result
        };

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    /// Saves a query for reuse (upsert: same name overwrites existing).
    ///
    /// Variables: {{x}} is replaced with the value as-is.
    /// Template quotes control JSON type:
    /// - "{{name}}" + {"name":"John"} → "John" (string)
    /// - {{age}} + {"age":"25"} → 25 (number)
    ///
    /// Variable values are always strings; template controls output type.
    #[tool]
    fn save_query(
        &self,
        Parameters(params): Parameters<SaveQueryParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        // Verify connection exists
        self.connections
            .get(&params.connection_name)
            .ok_or_else(|| self.connection_not_found(&params.connection_name))?;

        // Validate operation
        mongodb::QueryOperation::from_str(params.operation.as_str())
            .map_err(|e| rmcp::ErrorData::invalid_params(e.to_string(), None))?;

        let mut saved_queries = SavedQueries::load(&params.connection_name)
            .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;

        let is_update = saved_queries.get_query(&params.query_name).is_some();

        saved_queries.upsert_query(
            params.query_name.clone(),
            params.description,
            params.collection_name,
            params.operation.as_str().to_string(),
            params.query,
        );

        saved_queries
            .save(&params.connection_name)
            .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;

        let action = if is_update { "updated" } else { "saved" };
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Query '{}' {action} successfully in connection '{}'",
            params.query_name, params.connection_name
        ))]))
    }

    /// Lists all saved queries for a specific connection.
    ///
    /// Returns saved query objects with name, description, collection, operation,
    /// query, and timestamps. Use this to see available queries before running
    /// them with run_saved_query.
    #[tool]
    fn list_saved_queries(
        &self,
        Parameters(params): Parameters<ListSavedQueriesParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.connections
            .get(&params.connection_name)
            .ok_or_else(|| self.connection_not_found(&params.connection_name))?;

        let saved_queries = SavedQueries::load(&params.connection_name)
            .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;

        let response = serde_json::json!({
            "queries": saved_queries.queries,
            "count": saved_queries.queries.len()
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&response).unwrap(),
        )]))
    }

    /// Retrieves details of a specific saved query by name.
    ///
    /// Returns the full query definition including collection, operation, and query JSON.
    /// Useful to inspect a query before running it or to understand what it does.
    #[tool]
    fn get_saved_query(
        &self,
        Parameters(params): Parameters<GetSavedQueryParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.connections
            .get(&params.connection_name)
            .ok_or_else(|| self.connection_not_found(&params.connection_name))?;

        let saved_queries = SavedQueries::load(&params.connection_name)
            .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;

        let query = saved_queries.get_query(&params.query_name).ok_or_else(|| {
            rmcp::ErrorData::invalid_params(
                format!(
                    "Query '{}' not found in connection '{}'",
                    params.query_name, params.connection_name
                ),
                None,
            )
        })?;

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(query).unwrap(),
        )]))
    }

    /// Deletes a saved query permanently.
    ///
    /// WARNING: This action cannot be undone!
    #[tool]
    fn delete_saved_query(
        &self,
        Parameters(params): Parameters<DeleteSavedQueryParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.connections
            .get(&params.connection_name)
            .ok_or_else(|| self.connection_not_found(&params.connection_name))?;

        let mut saved_queries = SavedQueries::load(&params.connection_name)
            .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;

        if !saved_queries.delete_query(&params.query_name) {
            return Err(rmcp::ErrorData::invalid_params(
                format!(
                    "Query '{}' not found in connection '{}'",
                    params.query_name, params.connection_name
                ),
                None,
            ));
        }

        saved_queries
            .save(&params.connection_name)
            .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Query '{}' deleted successfully from connection '{}'",
            params.query_name, params.connection_name
        ))]))
    }

    /// Executes a previously saved query by name.
    ///
    /// Variables are always strings: {"age": "25"}
    /// Output type depends on template: {{age}} → 25, "{{age}}" → "25"
    ///
    /// Overrides (find only, ignored for other ops): limit, sort, projection.
    #[tool]
    async fn run_saved_query(
        &self,
        Parameters(params): Parameters<RunSavedQueryParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let connection = self
            .connections
            .get(&params.connection_name)
            .ok_or_else(|| self.connection_not_found(&params.connection_name))?;

        let saved_queries = SavedQueries::load(&params.connection_name)
            .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;

        let saved_query = saved_queries.get_query(&params.query_name).ok_or_else(|| {
            rmcp::ErrorData::invalid_params(
                format!(
                    "Query '{}' not found in connection '{}'",
                    params.query_name, params.connection_name
                ),
                None,
            )
        })?;

        // Substitute placeholders if variables provided
        let query = if let Some(ref variables) = params.variables {
            substitute_placeholders(&saved_query.query, variables).map_err(|missing| {
                rmcp::ErrorData::invalid_params(
                    format!(
                        "Missing required variables for query '{}': {}",
                        params.query_name,
                        missing.join(", ")
                    ),
                    None,
                )
            })?
        } else {
            // Check if query has placeholders that weren't provided
            let placeholders = find_placeholders(&saved_query.query);
            if !placeholders.is_empty() {
                return Err(rmcp::ErrorData::invalid_params(
                    format!(
                        "Query '{}' requires variables: {}",
                        params.query_name,
                        placeholders.into_iter().collect::<Vec<_>>().join(", ")
                    ),
                    None,
                ));
            }
            saved_query.query.clone()
        };

        tracing::info!(
            "Running saved query '{}' on connection '{}'",
            params.query_name,
            params.connection_name
        );

        let operation = mongodb::QueryOperation::from_str(&saved_query.operation)
            .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?;

        // Check for overrides on non-find operations
        let has_overrides =
            params.limit.is_some() || params.sort.is_some() || params.projection.is_some();
        let is_find = matches!(operation, mongodb::QueryOperation::Find);
        let warning = if has_overrides && !is_find {
            Some("Note: limit/sort/projection only apply to find operations (ignored)")
        } else {
            None
        };

        // Apply any runtime overrides (only effective for find)
        let options = QueryOptions {
            limit: params.limit,
            sort: params.sort,
            projection: params.projection,
            distinct_field: None,
        };

        let result = connection
            .execute_query(&saved_query.collection, &operation, &query, &options, 30)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format_error(&e), None))?;

        // Include warning if applicable
        let output = if let Some(warn) = warning {
            format!("{}\n\n{}", warn, result)
        } else {
            result
        };

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }
}

#[tool_handler]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: rmcp::model::Implementation {
                name: self.name.clone(),
                version: self.version.clone(),
                ..Default::default()
            },
            instructions: Some(
                "Read-only MongoDB query server. Workflow: \
                 1) list_connections to see available connections, \
                 2) get_data_model to understand the schema, \
                 3) list_collections to get exact collection names (case-sensitive!), \
                 4) query_mongodb to run queries. \
                 For time-based queries, use get_current_time first. \
                 Save reusable queries with save_query using {{placeholder}} variables, \
                 then run them with run_saved_query providing variable values."
                    .to_string(),
            ),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_placeholders() {
        let query = r#"{"userId": "{{userId}}", "date": {"$gte": "{{startDate}}"}}"#;
        let placeholders = find_placeholders(query);
        assert_eq!(placeholders.len(), 2);
        assert!(placeholders.contains("userId"));
        assert!(placeholders.contains("startDate"));
    }

    #[test]
    fn test_find_placeholders_empty() {
        let query = r#"{"status": "active"}"#;
        let placeholders = find_placeholders(query);
        assert!(placeholders.is_empty());
    }

    #[test]
    fn test_find_placeholders_duplicate() {
        let query = r#"{"a": "{{x}}", "b": "{{x}}"}"#;
        let placeholders = find_placeholders(query);
        assert_eq!(placeholders.len(), 1);
        assert!(placeholders.contains("x"));
    }

    #[test]
    fn test_substitute_placeholders_success() {
        let query = r#"{"userId": "{{userId}}", "active": true}"#;
        let mut vars = HashMap::new();
        vars.insert("userId".to_string(), "12345".to_string());

        let result = substitute_placeholders(query, &vars).unwrap();
        assert_eq!(result, r#"{"userId": "12345", "active": true}"#);
    }

    #[test]
    fn test_substitute_placeholders_missing() {
        let query = r#"{"userId": "{{userId}}", "date": "{{startDate}}"}"#;
        let mut vars = HashMap::new();
        vars.insert("userId".to_string(), "12345".to_string());

        let result = substitute_placeholders(query, &vars);
        assert!(result.is_err());
        let missing = result.unwrap_err();
        assert_eq!(missing.len(), 1);
        assert!(missing.contains(&"startDate".to_string()));
    }

    #[test]
    fn test_substitute_placeholders_no_placeholders() {
        let query = r#"{"status": "active"}"#;
        let vars = HashMap::new();

        let result = substitute_placeholders(query, &vars).unwrap();
        assert_eq!(result, query);
    }
}
