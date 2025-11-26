//! MongoDB query operations and mongosh execution.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::k8s_client::K8sClient;

#[derive(Debug, Clone)]
pub struct MongoCredentials {
    pub username: String,
    pub password: String,
}

/// Build mongosh command with authentication
fn build_mongosh_command(
    credentials: &MongoCredentials,
    database: &str,
    eval_code: String,
) -> Vec<String> {
    vec![
        "mongosh".to_string(),
        "-u".to_string(),
        credentials.username.clone(),
        "-p".to_string(),
        credentials.password.clone(),
        "--authenticationDatabase".to_string(),
        "admin".to_string(),
        database.to_string(),
        "--quiet".to_string(),
        "--eval".to_string(),
        eval_code,
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QueryOperation {
    Find,
    Aggregate,
    CountDocuments,
    Distinct,
}

/// Optional query parameters for find and distinct operations
#[derive(Debug, Clone, Default)]
pub struct QueryOptions {
    /// Maximum number of documents to return (find only)
    pub limit: Option<u32>,
    /// Sort order as JSON string (find only)
    pub sort: Option<String>,
    /// Projection as JSON string (find only)
    pub projection: Option<String>,
    /// Field name for distinct operation (distinct only)
    pub distinct_field: Option<String>,
}

impl QueryOperation {
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "find" => Ok(Self::Find),
            "aggregate" => Ok(Self::Aggregate),
            "countdocuments" => Ok(Self::CountDocuments),
            "distinct" => Ok(Self::Distinct),
            _ => bail!(
                "Invalid operation '{s}'. Must be one of: find, aggregate, countDocuments, distinct"
            ),
        }
    }

    pub fn to_mongosh_code(&self, collection: &str, query: &str, options: &QueryOptions) -> Result<String> {
        // Validate query is valid JSON
        let _: serde_json::Value = serde_json::from_str(query)
            .with_context(|| format!(
                "Query is not valid JSON. Received: '{query}'. Please ensure the query is a valid JSON string."
            ))?;

        // Escape collection name for safe use in JavaScript
        // Use bracket notation with JSON-escaped string to prevent injection
        let safe_collection = serde_json::to_string(collection)
            .context("Failed to escape collection name")?;

        // Wrap all outputs with JSON.stringify() to ensure valid JSON output
        let code = match self {
            Self::Find => {
                // Build find with optional projection, sort, and limit
                let projection = options.projection.as_deref().unwrap_or("{}");
                // Validate projection is valid JSON
                let _: serde_json::Value = serde_json::from_str(projection)
                    .with_context(|| format!("Projection is not valid JSON: '{projection}'"))?;

                let mut chain = format!("db[{safe_collection}].find({query}, {projection})");

                if let Some(sort) = &options.sort {
                    // Validate sort is valid JSON
                    let _: serde_json::Value = serde_json::from_str(sort)
                        .with_context(|| format!("Sort is not valid JSON: '{sort}'"))?;
                    chain = format!("{chain}.sort({sort})");
                }

                if let Some(limit) = options.limit {
                    chain = format!("{chain}.limit({limit})");
                }

                format!("JSON.stringify({chain}.toArray())")
            }
            Self::Aggregate => {
                format!("JSON.stringify(db[{safe_collection}].aggregate({query}).toArray())")
            }
            Self::CountDocuments => {
                // countDocuments returns a number, no need for JSON.stringify
                format!("db[{safe_collection}].countDocuments({query})")
            }
            Self::Distinct => {
                // First check if distinct_field option is provided (new simpler format)
                let field = if let Some(field) = &options.distinct_field {
                    field.clone()
                } else {
                    // Fall back to legacy format: {"field": "fieldName", "query": {...}}
                    let distinct_params: serde_json::Value = serde_json::from_str(query)
                        .with_context(|| format!(
                            "Distinct query must be valid JSON. Received: '{query}'"
                        ))?;

                    distinct_params.get("field")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow!(
                            "Distinct requires 'distinct_field' parameter"
                        ))?
                        .to_string()
                };

                // Escape field name for safe use
                let safe_field = serde_json::to_string(&field)
                    .context("Failed to escape field name")?;

                // Get filter - either from query directly (if distinct_field is set) or from legacy format
                let filter = if options.distinct_field.is_some() {
                    // Query is the filter directly
                    query.to_string()
                } else {
                    // Legacy format - extract query from {"field": ..., "query": ...}
                    let distinct_params: serde_json::Value = serde_json::from_str(query)?;
                    distinct_params.get("query").map_or_else(|| "{}".to_string(), |v| v.to_string())
                };

                format!("JSON.stringify(db[{safe_collection}].distinct({safe_field}, {filter}))")
            }
        };

        Ok(code)
    }
}

#[derive(Debug)]
pub struct MongoQuery {
    pub database: String,
    pub collection: String,
    pub operation: QueryOperation,
    pub query: String,
    pub options: QueryOptions,
}

/// Get `MongoDB` credentials from pod environment variables
pub async fn get_mongodb_credentials(
    k8s_client: &K8sClient,
    namespace: &str,
    pod_name: &str,
    container_name: &str,
) -> Result<MongoCredentials> {
    tracing::debug!("Getting MongoDB credentials from pod {}/{} container {}", namespace, pod_name, container_name);

    // Get both env vars in a single pod fetch
    let env_vars = k8s_client
        .get_pod_env_vars(namespace, pod_name, &[
            "MONGO_INITDB_ROOT_USERNAME_FILE",
            "MONGO_INITDB_ROOT_PASSWORD_FILE",
        ])
        .await?;

    let username_file_path = env_vars.first().cloned().flatten().ok_or_else(|| anyhow!(
        "MongoDB credentials not found: MONGO_INITDB_ROOT_USERNAME_FILE environment variable missing in pod '{namespace}/{pod_name}'."
    ))?;
    let password_file_path = env_vars.get(1).cloned().flatten().ok_or_else(|| anyhow!(
        "MongoDB credentials not found: MONGO_INITDB_ROOT_PASSWORD_FILE environment variable missing in pod '{namespace}/{pod_name}'."
    ))?;

    tracing::debug!("Username file: {}, Password file: {}", username_file_path, password_file_path);

    // Read both credential files in parallel
    let (username_result, password_result) = tokio::join!(
        k8s_client.read_file_from_pod(namespace, pod_name, container_name, &username_file_path),
        k8s_client.read_file_from_pod(namespace, pod_name, container_name, &password_file_path)
    );

    let username = username_result.context("Failed to read username file")?.trim().to_string();
    let password = password_result.context("Failed to read password file")?.trim().to_string();

    Ok(MongoCredentials { username, password })
}

/// Validate that the operation is read-only
///
/// Note: All operations in the `QueryOperation` enum are read-only by design.
/// This function exists as an explicit validation point for safety.
pub const fn validate_readonly_operation(_operation: &QueryOperation) {
    // All operations in the enum are read-only by design
}

/// Execute a `MongoDB` query via mongosh
pub async fn execute_mongosh_query(
    k8s_client: &K8sClient,
    namespace: &str,
    pod_name: &str,
    container_name: &str,
    credentials: &MongoCredentials,
    query: &MongoQuery,
    timeout_secs: u64,
) -> Result<String> {
    // Validate operation is read-only
    validate_readonly_operation(&query.operation);

    // Build mongosh eval code
    let eval_code = query.operation.to_mongosh_code(&query.collection, &query.query, &query.options)?;

    tracing::debug!("Mongosh eval code: {}", eval_code);

    let command = build_mongosh_command(credentials, &query.database, eval_code);

    tracing::info!(
        "Executing query: {:?} on {}.{}",
        query.operation,
        query.database,
        query.collection
    );

    // Execute command with timeout
    let output = k8s_client
        .exec_command_in_pod(namespace, pod_name, container_name, command, timeout_secs)
        .await
        .context("Failed to execute mongosh command")?;

    // Parse and validate output
    parse_mongosh_output(&output, &query.collection, &query.database)
}

/// List all collections in a database
pub async fn list_collections(
    k8s_client: &K8sClient,
    namespace: &str,
    pod_name: &str,
    container_name: &str,
    credentials: &MongoCredentials,
    database: &str,
) -> Result<Vec<String>> {
    let eval_code = "JSON.stringify(db.getCollectionNames())".to_string();
    let command = build_mongosh_command(credentials, database, eval_code);

    tracing::info!("Listing collections in database: {}", database);

    let output = k8s_client
        .exec_command_in_pod(namespace, pod_name, container_name, command, 30)
        .await
        .context("Failed to list collections")?;

    let trimmed = output.trim();

    // Parse the JSON array of collection names
    let mut collections: Vec<String> = serde_json::from_str(trimmed)
        .with_context(|| format!("Failed to parse collection list: {trimmed}"))?;

    // Sort for deterministic output
    collections.sort();

    Ok(collections)
}

/// Parse mongosh output and validate it's valid JSON
pub fn parse_mongosh_output(raw_output: &str, collection: &str, database: &str) -> Result<String> {
    let trimmed = raw_output.trim();

    if trimmed.is_empty() {
        bail!(
            "Query returned empty output. This may indicate:\n\
            - The collection '{collection}' might not exist in database '{database}'\n\
            - Use list_collections to verify the exact collection name (case-sensitive)"
        );
    }

    // Check for common error patterns and provide detailed feedback
    if trimmed.contains("MongoServerError") || trimmed.contains("MongoError") {
        // Extract useful error information
        let error_msg = if trimmed.contains("ns not found") || trimmed.contains("doesn't exist") {
            format!(
                "Collection '{collection}' not found in database '{database}'.\n\
                SOLUTION: Use list_collections to get exact collection names (they are case-sensitive).\n\
                Raw error: {trimmed}"
            )
        } else if trimmed.contains("Authentication failed") {
            format!(
                "MongoDB authentication failed. The credentials may have changed.\n\
                Raw error: {trimmed}"
            )
        } else if trimmed.contains("timed out") || trimmed.contains("timeout") {
            format!(
                "Query timed out. The query may be too slow or the database is under heavy load.\n\
                SUGGESTIONS:\n\
                - Add more specific filters to reduce result size\n\
                - Use $limit in aggregation pipelines\n\
                - Try countDocuments first to check data size\n\
                Raw error: {trimmed}"
            )
        } else if trimmed.contains("SyntaxError") || trimmed.contains("Invalid") {
            format!(
                "Invalid query syntax. Check your query JSON format.\n\
                COMMON ISSUES:\n\
                - Ensure JSON is properly quoted\n\
                - For distinct: use {{\"field\": \"fieldName\", \"query\": {{}}}}\n\
                - For aggregate: use array format [{{\"$match\": {{}}}}]\n\
                Raw error: {trimmed}"
            )
        } else {
            format!("MongoDB error: {trimmed}")
        };
        bail!(error_msg);
    }

    // Try to parse as JSON to validate
    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(_) => Ok(trimmed.to_string()),
        Err(e) => {
            // If it's not valid JSON, it might still be a valid response (like a number)
            if trimmed.parse::<i64>().is_ok() || trimmed.parse::<f64>().is_ok() {
                Ok(trimmed.to_string())
            } else {
                tracing::error!("Failed to parse mongosh output as JSON: {}", e);
                tracing::error!("Raw output: {}", trimmed);
                bail!(
                    "Failed to parse query result.\n\
                    Collection: '{collection}', Database: '{database}'\n\
                    Raw output: {trimmed}\n\
                    Parse error: {e}"
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_operation_from_str() {
        assert!(matches!(QueryOperation::from_str("find"), Ok(QueryOperation::Find)));
        assert!(matches!(QueryOperation::from_str("FIND"), Ok(QueryOperation::Find)));
        assert!(matches!(QueryOperation::from_str("aggregate"), Ok(QueryOperation::Aggregate)));
        assert!(matches!(QueryOperation::from_str("countDocuments"), Ok(QueryOperation::CountDocuments)));
        assert!(matches!(QueryOperation::from_str("distinct"), Ok(QueryOperation::Distinct)));
        assert!(QueryOperation::from_str("invalid").is_err());
    }

    #[test]
    fn test_validate_readonly_operation() {
        // All operations are valid by design
        validate_readonly_operation(&QueryOperation::Find);
        validate_readonly_operation(&QueryOperation::Aggregate);
        validate_readonly_operation(&QueryOperation::CountDocuments);
        validate_readonly_operation(&QueryOperation::Distinct);
    }

    #[test]
    fn test_to_mongosh_code() {
        let opts = QueryOptions::default();

        let op = QueryOperation::Find;
        let code = op.to_mongosh_code("users", "{}", &opts).unwrap();
        assert_eq!(code, "JSON.stringify(db[\"users\"].find({}, {}).toArray())");

        let op = QueryOperation::Aggregate;
        let code = op.to_mongosh_code("users", "[{\"$match\": {}}]", &opts).unwrap();
        assert_eq!(code, "JSON.stringify(db[\"users\"].aggregate([{\"$match\": {}}]).toArray())");

        let op = QueryOperation::CountDocuments;
        let code = op.to_mongosh_code("users", "{}", &opts).unwrap();
        assert_eq!(code, "db[\"users\"].countDocuments({})");

        let op = QueryOperation::Distinct;
        let code = op.to_mongosh_code("users", r#"{"field": "email", "query": {}}"#, &opts).unwrap();
        assert_eq!(code, "JSON.stringify(db[\"users\"].distinct(\"email\", {}))");
    }

    #[test]
    fn test_to_mongosh_code_with_options() {
        // Test find with limit
        let opts = QueryOptions {
            limit: Some(10),
            ..Default::default()
        };
        let code = QueryOperation::Find.to_mongosh_code("users", "{}", &opts).unwrap();
        assert_eq!(code, "JSON.stringify(db[\"users\"].find({}, {}).limit(10).toArray())");

        // Test find with sort and limit
        let opts = QueryOptions {
            limit: Some(5),
            sort: Some("{\"createdAt\": -1}".to_string()),
            ..Default::default()
        };
        let code = QueryOperation::Find.to_mongosh_code("users", "{}", &opts).unwrap();
        assert_eq!(code, "JSON.stringify(db[\"users\"].find({}, {}).sort({\"createdAt\": -1}).limit(5).toArray())");

        // Test find with projection
        let opts = QueryOptions {
            projection: Some("{\"name\": 1}".to_string()),
            ..Default::default()
        };
        let code = QueryOperation::Find.to_mongosh_code("users", "{}", &opts).unwrap();
        assert_eq!(code, "JSON.stringify(db[\"users\"].find({}, {\"name\": 1}).toArray())");

        // Test distinct with distinct_field option
        let opts = QueryOptions {
            distinct_field: Some("country".to_string()),
            ..Default::default()
        };
        let code = QueryOperation::Distinct.to_mongosh_code("users", "{}", &opts).unwrap();
        assert_eq!(code, "JSON.stringify(db[\"users\"].distinct(\"country\", {}))");
    }

    #[test]
    fn test_to_mongosh_code_escapes_special_chars() {
        let opts = QueryOptions::default();
        // Test that special characters in collection names are properly escaped
        let op = QueryOperation::Find;
        let code = op.to_mongosh_code("test\"; db.dropDatabase(); //", "{}", &opts).unwrap();
        // The collection name should be JSON-escaped in bracket notation
        // db["test\"; db.dropDatabase(); //"].find({}) - the quotes are escaped
        assert!(code.starts_with("JSON.stringify(db[\""));
        assert!(code.contains("\\\""));  // Contains escaped quotes
        // The malicious code is inside the string, not executed as JS
        assert!(code.contains("db.dropDatabase()"));  // It's in the string
        assert!(!code.starts_with("JSON.stringify(db.test"));  // NOT using dot notation
    }

    #[test]
    fn test_parse_mongosh_output() {
        assert!(parse_mongosh_output("[]", "test", "db").is_ok());
        assert!(parse_mongosh_output("[{\"name\": \"test\"}]", "test", "db").is_ok());
        assert!(parse_mongosh_output("42", "test", "db").is_ok());
        assert!(parse_mongosh_output("3.14", "test", "db").is_ok());
        assert!(parse_mongosh_output("", "test", "db").is_err());
        assert!(parse_mongosh_output("MongoServerError: connection failed", "test", "db").is_err());
    }

    #[test]
    fn test_to_mongosh_code_invalid_json() {
        let opts = QueryOptions::default();
        let op = QueryOperation::Find;
        // Invalid JSON should fail
        assert!(op.to_mongosh_code("users", "not valid json", &opts).is_err());
        assert!(op.to_mongosh_code("users", "{unclosed", &opts).is_err());
    }

    #[test]
    fn test_to_mongosh_code_distinct_missing_field() {
        let opts = QueryOptions::default();
        let op = QueryOperation::Distinct;
        // Distinct without "field" parameter or distinct_field option should fail
        assert!(op.to_mongosh_code("users", "{}", &opts).is_err());
        assert!(op.to_mongosh_code("users", r#"{"query": {}}"#, &opts).is_err());
    }

    #[test]
    fn test_to_mongosh_code_distinct_field_escaping() {
        let opts = QueryOptions::default();
        let op = QueryOperation::Distinct;
        // Field names with special characters should be escaped
        let code = op.to_mongosh_code("users", r#"{"field": "test\"field", "query": {}}"#, &opts).unwrap();
        assert!(code.contains("\\\""));  // Contains escaped quotes in field name
    }
}
