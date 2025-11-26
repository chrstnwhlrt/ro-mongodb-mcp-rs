//! MCP tool parameter types.
//! These structs are deserialized by rmcp macros but not directly constructed.

use schemars::JsonSchema;
use serde::Deserialize;

/// Parameters for get_data_model tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetDataModelParams {
    /// The connection name from list_connections. Case-sensitive.
    pub connection_name: String,
}

/// Parameters for list_collections tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListCollectionsParams {
    /// The connection name from list_connections. Case-sensitive.
    pub connection_name: String,
}

/// The query operation type
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum QueryOperationType {
    /// Query documents matching a filter. Returns array of documents.
    Find,
    /// Run an aggregation pipeline. Query must be a JSON array of pipeline stages.
    Aggregate,
    /// Count documents matching a filter. Returns a number.
    CountDocuments,
    /// Get unique values for a field. Use with distinct_field parameter.
    Distinct,
}

impl QueryOperationType {
    /// Convert to the string format expected by mongodb::QueryOperation::from_str
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Find => "find",
            Self::Aggregate => "aggregate",
            Self::CountDocuments => "countDocuments",
            Self::Distinct => "distinct",
        }
    }
}

/// Parameters for query_mongodb tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct QueryMongodbParams {
    /// The connection name from list_connections. Case-sensitive.
    pub connection_name: String,
    /// The collection name from list_collections. Case-sensitive.
    pub collection_name: String,
    /// The query operation type: find, aggregate, countDocuments, distinct
    pub operation: QueryOperationType,
    /// JSON string: filter {} for find/countDocuments/distinct, pipeline [] for aggregate.
    pub query: String,
    /// (find only) Maximum number of documents to return. Recommended for large collections.
    #[serde(default)]
    pub limit: Option<u32>,
    /// (find only) Sort order as JSON object. Example: {"createdAt": -1} for descending.
    #[serde(default)]
    pub sort: Option<String>,
    /// (find only) Fields to include/exclude as JSON object. Example: {"name": 1, "email": 1} or {"password": 0}.
    #[serde(default)]
    pub projection: Option<String>,
    /// (distinct) REQUIRED. Field to get unique values from. Query param becomes the filter.
    #[serde(default)]
    pub distinct_field: Option<String>,
}

/// Parameters for save_query tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SaveQueryParams {
    /// The connection name from list_connections. Case-sensitive.
    pub connection_name: String,
    /// Unique name for this query (e.g., 'active_users'). If exists, will be updated.
    pub query_name: String,
    /// Clear description of what this query does.
    pub description: String,
    /// The collection name from list_collections. Case-sensitive.
    pub collection_name: String,
    /// The query operation type (find, aggregate, countDocuments, distinct).
    pub operation: QueryOperationType,
    /// The query JSON string. Supports {{placeholder}} variables for runtime substitution.
    /// Use quotes for strings: {"name": "{{name}}"}
    /// Omit quotes for numbers/arrays: {"_id": {{userId}}, "tags": {{tagList}}}
    pub query: String,
}

/// Parameters for list_saved_queries tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListSavedQueriesParams {
    /// The connection name from list_connections. Case-sensitive.
    pub connection_name: String,
}

/// Parameters for get_saved_query tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetSavedQueryParams {
    /// The connection name from list_connections. Case-sensitive.
    pub connection_name: String,
    /// The name of the saved query to retrieve.
    pub query_name: String,
}

/// Parameters for delete_saved_query tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteSavedQueryParams {
    /// The connection name from list_connections. Case-sensitive.
    pub connection_name: String,
    /// The name of the saved query to delete.
    pub query_name: String,
}

/// Parameters for run_saved_query tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunSavedQueryParams {
    /// The connection name from list_connections. Case-sensitive.
    pub connection_name: String,
    /// The name of the saved query to execute.
    pub query_name: String,
    /// Variables: {"userId": "123"} replaces {{userId}} → 123 or "{{userId}}" → "123".
    /// Template quotes control type: {{x}} = raw value, "{{x}}" = string.
    #[serde(default)]
    pub variables: Option<std::collections::HashMap<String, String>>,
    /// (find only) Override: Maximum number of documents to return.
    #[serde(default)]
    pub limit: Option<u32>,
    /// (find only) Override: Sort order as JSON object. Example: {"createdAt": -1}.
    #[serde(default)]
    pub sort: Option<String>,
    /// (find only) Override: Fields to include/exclude as JSON object.
    #[serde(default)]
    pub projection: Option<String>,
}
