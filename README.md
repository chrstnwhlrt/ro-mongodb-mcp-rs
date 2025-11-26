# ro-mongodb-mcp-rs

A high-performance [Model Context Protocol (MCP)](https://modelcontextprotocol.io/) server for executing **read-only MongoDB queries**. Built in Rust for speed and reliability.

## Overview

This server enables LLMs to safely query MongoDB databases through the MCP protocol. It supports two connection types:

| Connection Type | Use Case | Speed |
|-----------------|----------|-------|
| **Direct URL** | Local, Atlas, any network-accessible MongoDB | ~13ms |
| **Kubernetes** | MongoDB running in K8s pods | ~700ms |

## Features

- **Read-only queries** - Only `find`, `aggregate`, `countDocuments`, and `distinct` operations
- **Query controls** - Limit, sort, and projection parameters for find operations
- **Dual connection support** - Kubernetes pods and direct MongoDB URLs
- **Saved queries with variables** - Save reusable queries with `{{placeholder}}` variables
- **Schema integration** - Data model files help LLMs understand your collections
- **Auto-discovery** - Automatic K8s credential discovery from pod environment
- **Timeout protection** - 30-second query timeout prevents runaway queries
- **Fast startup** - ~4ms cold start, sub-millisecond for cached operations

## Quick Start

### 1. Build

```bash
cargo build --release
```

### 2. Configure

Create `~/.config/ro-mongodb-mcp-rs/config.yaml`:

```yaml
# Direct connection (simplest)
connections:
  - name: local
    mongodb_url: mongodb://localhost:27017
    database_name: mydb
    data_model_file_path: /path/to/schema.md
```

Or copy and customize the example:

```bash
cp config.example.yaml ~/.config/ro-mongodb-mcp-rs/config.yaml
```

### 3. Run

```bash
./target/release/ro-mongodb-mcp-rs
```

The server communicates via JSON-RPC 2.0 over stdin/stdout.

## Configuration

Configuration file: `~/.config/ro-mongodb-mcp-rs/config.yaml`

### Direct MongoDB Connections

For local development, MongoDB Atlas, or any network-accessible MongoDB:

```yaml
connections:
  - name: local-dev
    mongodb_url: mongodb://localhost:27017
    database_name: myapp
    data_model_file_path: /path/to/schema.md

  - name: atlas-prod
    mongodb_url: mongodb+srv://user:pass@cluster.mongodb.net
    database_name: production
    data_model_file_path: /path/to/schema.md
```

### Kubernetes Namespace Connections

For MongoDB running in Kubernetes clusters:

```yaml
# Optional: custom kubeconfig
# kubeconfig_path: /path/to/kubeconfig

namespaces:
  - namespace_name: production      # K8s namespace (also the connection name)
    deployment_name: mongodb        # Pod label: app=mongodb
    database_name: myapp
    data_model_file_path: /path/to/schema.md
```

**K8s Credential Discovery:** The server reads credentials from pod environment variables:
- `MONGO_INITDB_ROOT_USERNAME_FILE` → file path containing username
- `MONGO_INITDB_ROOT_PASSWORD_FILE` → file path containing password

### Configuration Fields

| Field | Description |
|-------|-------------|
| `name` / `namespace_name` | Unique connection identifier |
| `mongodb_url` | MongoDB connection string (direct connections only) |
| `deployment_name` | Pod label selector `app=<value>` (K8s only) |
| `database_name` | Default database for queries |
| `data_model_file_path` | Schema documentation file (any format) |

**Important:** Connection names must be unique across all connections.

## MCP Tools

The server provides 10 tools:

### Discovery Tools

| Tool | Description |
|------|-------------|
| `list_connections` | List all configured connections |
| `list_collections` | List MongoDB collections (case-sensitive names) |
| `get_data_model` | Get schema documentation for a connection |
| `get_current_time` | Get current timestamp for time-based queries |

### Query Tools

| Tool | Description |
|------|-------------|
| `query_mongodb` | Execute a read-only MongoDB query |

**Supported operations:**

```javascript
// find - retrieve documents
{"status": "active"}

// aggregate - pipeline queries
[{"$match": {}}, {"$group": {"_id": "$status", "count": {"$sum": 1}}}]

// countDocuments - count matching documents
{"status": "active"}

// distinct - unique values (use distinct_field param)
{}  // with distinct_field: "country"
```

**Optional parameters (find only):**

| Parameter | Description | Example |
|-----------|-------------|---------|
| `limit` | Maximum documents to return | `10` |
| `sort` | Sort order (JSON) | `{"createdAt": -1}` |
| `projection` | Fields to include/exclude | `{"name": 1, "email": 1}` |
| `distinct_field` | Field for distinct values | `"country"` |

### Saved Query Tools

| Tool | Description |
|------|-------------|
| `save_query` | Save a query for later reuse |
| `list_saved_queries` | List all saved queries for a connection |
| `get_saved_query` | Get details of a saved query |
| `run_saved_query` | Execute a saved query |
| `delete_saved_query` | Delete a saved query |

**Placeholder Variables:** Saved queries support `{{placeholder}}` syntax for runtime substitution:

```json
// Save with placeholders - quotes in template control output type
{
  "query": "{\"name\": \"{{name}}\", \"age\": {{age}}}"
}
//         "{{name}}" → string     {{age}} → number

// Run with variables (all values are strings)
{
  "variables": {"name": "John", "age": "25"}
}
// Result: {"name": "John", "age": 25}
```

**Runtime Overrides:** For find operations only, you can override `limit`, `sort`, and `projection`. These are ignored for other operations (with a warning).

**Storage:** Queries are persisted per connection in `~/.local/share/ro-mongodb-mcp-rs/<connection>.queries.yaml`

## Usage Examples

### Basic Query

```json
{
  "name": "query_mongodb",
  "arguments": {
    "connection_name": "local",
    "collection_name": "users",
    "operation": "find",
    "query": "{\"status\": \"active\"}",
    "limit": 10,
    "sort": "{\"createdAt\": -1}",
    "projection": "{\"name\": 1, \"email\": 1}"
  }
}
```

### Aggregation Pipeline

```json
{
  "name": "query_mongodb",
  "arguments": {
    "connection_name": "local",
    "collection_name": "orders",
    "operation": "aggregate",
    "query": "[{\"$match\": {\"status\": \"completed\"}}, {\"$group\": {\"_id\": \"$userId\", \"total\": {\"$sum\": \"$amount\"}}}]"
  }
}
```

### Count Documents

```json
{
  "name": "query_mongodb",
  "arguments": {
    "connection_name": "local",
    "collection_name": "users",
    "operation": "countDocuments",
    "query": "{}"
  }
}
```

### Distinct Values

```json
{
  "name": "query_mongodb",
  "arguments": {
    "connection_name": "local",
    "collection_name": "users",
    "operation": "distinct",
    "query": "{}",
    "distinct_field": "country"
  }
}
```

### Save a Query (with Variables)

```json
{
  "name": "save_query",
  "arguments": {
    "connection_name": "local",
    "query_name": "user_activity",
    "description": "Get user activity after a date",
    "collection_name": "events",
    "operation": "find",
    "query": "{\"userId\": \"{{userId}}\", \"createdAt\": {\"$gte\": \"{{startDate}}\"}}"
  }
}
```

### Run a Saved Query (with Variables)

```json
{
  "name": "run_saved_query",
  "arguments": {
    "connection_name": "local",
    "query_name": "user_activity",
    "variables": {"userId": "12345", "startDate": "2024-01-01T00:00:00Z"},
    "limit": 100
  }
}
```

## Performance

Benchmarks on typical hardware:

| Metric | Time |
|--------|------|
| Binary startup | 4ms |
| `get_current_time` | <1ms |
| `list_connections` | <1ms |
| Direct MongoDB query | ~13ms |
| K8s MongoDB query | ~700ms |
| 10 sequential direct queries | 5ms (connection reused) |

Binary size: ~21MB (release build)

## MCP Client Integration

### Claude Desktop

Add to your Claude Desktop config:

```json
{
  "mcpServers": {
    "mongodb": {
      "command": "/path/to/ro-mongodb-mcp-rs"
    }
  }
}
```

### Goose CLI

Add to `~/.config/goose/config.yaml`:

```yaml
extensions:
  mongodb:
    bundled: false
    display_name: "MongoDB"
    enabled: true
    name: "mongodb"
    timeout: 300
    type: "stdio"
    cmd: "/path/to/ro-mongodb-mcp-rs"
    args: []
```

Or add it interactively with `goose configure` and select "Command-line Extension".

### Generic MCP Client

The server uses JSON-RPC 2.0 over stdin/stdout:

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' | ./ro-mongodb-mcp-rs
```

## Development

### Prerequisites

- Rust 1.85 or later (edition 2024)
- For K8s connections: valid kubeconfig with cluster access
- For direct connections: network access to MongoDB

### Build

```bash
cargo build --release
```

### Test

```bash
cargo test
```

### Debug Logging

```bash
RUST_LOG=ro_mongodb_mcp_rs=debug ./target/release/ro-mongodb-mcp-rs
```

### Lint

```bash
cargo clippy --all-targets
```

## Version Management

Check version:

```bash
./ro-mongodb-mcp-rs --version
```

Release new version:

```bash
./set-version.sh 1.2.3
git push origin main --follow-tags
```

## Troubleshooting

### "No healthy pods found" (Kubernetes)

```bash
# Check pods exist and are running
kubectl get pods -n <namespace> -l app=<deployment_name>

# Verify pod is healthy
kubectl describe pod <pod-name> -n <namespace>
```

### "MongoDB credential environment variable not found" (Kubernetes)

```bash
# Check pod environment variables
kubectl exec -n <namespace> <pod-name> -- env | grep MONGO
```

Required variables:
- `MONGO_INITDB_ROOT_USERNAME_FILE`
- `MONGO_INITDB_ROOT_PASSWORD_FILE`

### "Failed to read data model file"

- Ensure `data_model_file_path` is an absolute path
- Verify the file exists: `ls -la /path/to/schema.md`

### Connection timeout (Direct)

- Verify MongoDB is running: `mongosh mongodb://localhost:27017`
- Check network connectivity
- Verify credentials in URL are correct

### Query timeout

Queries timeout after 30 seconds. For large datasets:
- Add filters to reduce result size
- Use `$limit` in aggregation pipelines
- Use `countDocuments` first to check data size

## Project Structure

```
src/
├── main.rs              # Entry point, CLI, initialization
├── config.rs            # Configuration loading and validation
├── connection.rs        # MongoConnection trait and registry
├── direct_connection.rs # Direct MongoDB URL connections
├── k8s_connection.rs    # Kubernetes namespace connections
├── k8s_client.rs        # Kubernetes API interactions
├── mcp.rs               # MCP server and tool implementations
├── mongodb.rs           # Query operations and mongosh execution
├── saved_queries.rs     # Query persistence
└── tools.rs             # MCP tool parameter types
```

## Security

- **Read-only by design** - Only read operations are supported
- **No query injection** - Operations are validated before execution
- **Credential isolation** - K8s credentials stay in the cluster
- **Timeout protection** - 30-second limit prevents resource exhaustion

**Note:** Direct connection URLs may contain credentials. Keep your config file secure:

```bash
chmod 600 ~/.config/ro-mongodb-mcp-rs/config.yaml
```

## License

MIT License. See [LICENSE](LICENSE) for details.
