# Migration from opcua to async-opcua

This document outlines the steps required to migrate from the `opcua` crate to the `async-opcua` crate in the Chirpstack OPC UA Gateway.

## Background

The `opcua` crate is being replaced with `async-opcua` to take advantage of the latter's improved async support, better integration with Tokio, and more modern API design.

## Migration Steps

### 0. Create a Dedicated Branch

### 1. Incremental Migration Steps

Follow these minimal steps, verifying compilation at each stage:

#### Step 1: Dependency Update
```bash
# In Cargo.toml:
[dependencies]
# Remove/comment out:
# opcua = "0.12.0"  
async-opcua = { version = "^0.14", features = ["server"] }

# Clean and verify
cargo clean
cargo check # Will fail but shows needed changes
```

#### Step 2: Update Imports Only
```rust
// In src/opc_ua.rs:
// Replace:
use opcua::server::{Server, ServerBuilder};
// With:
use async_opcua::server::prelude::*;
// Keep other imports for now

cargo check # Verify compilation progresses
```

#### Step 3: Minimal Struct Changes
```rust
// Change:
pub struct OpcUa {
    pub server: Arc<RwLock<Server>>,
    // ...
}
// To:
pub struct OpcUa {
    pub server: Server,
    // ...
}
// Remove server_config field

cargo check
```

#### Step 4: Basic Server Creation
```rust
let server = Server::new(server_config);
let ns = server.register_namespace("urn:chirpstack:opcua")
    .expect("Namespace registration failed");

// Temporary debug output
println!("Server created with namespace index: {}", ns);

cargo check
```

#### Step 5: Minimal Run Method
```rust
pub async fn run(&self) -> Result<(), OpcGwError> {
    // Temporarily comment out populate_address_space()
    self.server.run().await.map_err(|e| {
        OpcGwError::OpcUaError(format!("Server run failed: {}", e))
    })
}

cargo check
```

#### Step 6: Basic Address Space Setup
```rust
async fn init_address_space(&self) -> Result<(), OpcGwError> {
    let address_space = self.server.address_space();
    let objects = NodeId::objects_folder();
    
    // Test folder
    let folder_id = NodeId::new(self.ns, "test_folder");
    address_space.add_folder(&folder_id, "Test", &objects).await?;
    
    // Test variable 
    let var_id = NodeId::new(self.ns, "test_var");
    address_space.add_variable(&var_id, "TestVar", &folder_id, Variant::Float(0.0)).await?;
    
    Ok(())
}

cargo check
```

#### Step 7: Single Application Folder
```rust
let app_node_id = NodeId::new(self.ns, "test_app");
address_space.add_folder(
    &app_node_id,
    "TestApp",
    &objects_folder,
).await?;

cargo check
```

#### Step 8: Single Test Variable
```rust
async fn add_test_variable(&self) -> Result<(), OpcGwError> {
    let node_id = NodeId::new(self.ns, "test_var");
    self.server.address_space().add_variable(
        &node_id,
        "TestVariable",
        &NodeId::objects_folder(),
        Variant::Float(0.0),
    ).await?;
    Ok(())
}

cargo check
```

#### Step 9: Final Verification
```bash
# Basic smoke test
cargo run &  # Start server in background
sleep 2      # Wait for server startup
opcua-client -u opc.tcp://localhost:4840  # Verify connection
kill %1      # Stop server

# Run tests
cargo test

# Cleanup old dependencies
# Remove from Cargo.toml:
# opcua = "0.12.0"
cargo update
```

### 2. Create a Dedicated Branch

Before making any changes, create a dedicated branch for the migration:

```bash
# Ensure you're on the main branch and it's up to date
git checkout main
git pull

# Create a new branch for the async-opcua migration
git checkout -b async-opcua

# Verify you're on the new branch
git branch
```

All subsequent changes should be made on this `async-opcua` branch to keep the migration isolated until it's ready to be merged.

### 3. Update Dependencies (Detailed)

First, update the dependencies in `Cargo.toml`:

```toml
[dependencies]
# Remove or comment out the opcua dependency
# opcua = "0.12.0"
# opcua = {git = "https://github.com/locka99/opcua.git" }

# Add the async-opcua dependency
async-opcua = { version = "0.14.0", features = ["server"] }
```

### 4. Complete Server Implementation
After incremental steps are working:

The most significant changes are in the `src/opc_ua.rs` file:

#### 2.1. Update Imports

Replace the `opcua` imports with `async-opcua` imports:

```rust
// Old imports
use opcua::server::{Server, ServerBuilder};
use opcua::sync::RwLock;
use opcua::types::variant::Variant::Float;
use opcua::types::DataTypeId::Integer;
use opcua::types::VariableId::OperationLimitsType_MaxNodesPerTranslateBrowsePathsToNodeIds;

// New imports
use async_opcua::server::{
    prelude::*,
    server::Server,
    config::ServerConfig,
    address_space::{
        types::{DataValue, Variant},
        node::NodeId,
        variable::Variable,
    },
};
```

#### 2.2. Update OpcUa Struct

Modify the `OpcUa` struct to use the new `async-opcua` types:

```rust
pub struct OpcUa {
    /// Application configuration parameters
    pub config: AppConfig,
    /// opc ua server instance
    pub server: Server,
    /// Index of the opc ua address space
    pub ns: u16,
    /// Metrics list
    pub storage: Arc<std::sync::Mutex<Storage>>,
}
```

Key changes:
- Replace `Arc<RwLock<Server>>` with `Server`
- Remove the `server_config` field as it's not needed after initialization

#### 2.3. Update Server Creation

Modify the `new` method to create a server using the `async-opcua` API:

```rust
pub fn new(config: &AppConfig, storage: Arc<std::sync::Mutex<Storage>>) -> Self {
    trace!("New OPC UA structure");
    
    // Create server configuration using hardcoded values initially
    let server_config = Self::create_server_config(&config.opcua.config_file.clone());
    
    // Create a server instance
    let server = Server::new(server_config);
    
    // Register the namespace in the OPC UA server
    let ns = server.register_namespace(OPCUA_ADDRESS_SPACE)
        .expect("Failed to register namespace");
    
    // Return the new OpcUa structure
    OpcUa {
        config: config.clone(),
        server,
        ns,
        storage,
    }
}
```

#### 2.4. Create Hardcoded Server Configuration

Initially, use a hardcoded configuration to simplify the migration:

```rust
fn create_server_config(config_file_name: &String) -> ServerConfig {
    debug!("Creating server config with hardcoded values");
    trace!("(Ignoring config file: {:?} for now)", config_file_name);
    
    // Create a default configuration
    let mut config = ServerConfig::default();
    
    // Set the server name and URI
    config.application_name = "Chirpstack OPC UA Gateway".to_string();
    config.application_uri = "urn:chirpstack:opcua:gateway".to_string();
    
    // Set the endpoint URL using local IP
    if let Ok(my_ip_address) = local_ip() {
        config.endpoints.push(format!("opc.tcp://{}:4840", my_ip_address));
    } else {
        config.endpoints.push("opc.tcp://0.0.0.0:4840".to_string());
        warn!("Failed to get local IP address, using 0.0.0.0");
    }
    
    // Set security policies (none for simplicity in the first step)
    config.security_policies = vec!["None".to_string()];
    
    // Set user token policies (anonymous for simplicity in the first step)
    config.user_token_policies = vec!["anonymous".to_string()];
    
    // Set discovery URL
    if let Ok(my_ip_address) = local_ip() {
        config.discovery_urls = vec![format!("opc.tcp://{}:4840", my_ip_address)];
    } else {
        config.discovery_urls = vec!["opc.tcp://0.0.0.0:4840".to_string()];
    }
    
    // Set product URI
    config.product_uri = "urn:chirpstack:opcua:gateway:product".to_string();
    
    // Set server capabilities
    config.server_capabilities = vec!["DA".to_string()];
    
    // Return the configuration
    config
}
```

#### 2.5. Update the Run Method

Modify the `run` method to use the async API:

```rust
pub async fn run(&self) -> Result<(), OpcGwError> {
    debug!("Running OPC UA server");
    
    // Populate the address space
    self.populate_address_space().await?;
    
    // Start the server
    match self.server.run().await {
        Ok(_) => Ok(()),
        Err(e) => Err(OpcGwError::OpcUaError(format!("Failed to run OPC UA server: {}", e))),
    }
}
```

#### 2.6. Update Address Space Population

Rewrite the address space population to use the async API:

```rust
async fn populate_address_space(&self) -> Result<(), OpcGwError> {
    debug!("Populating address space");
    
    let address_space = self.server.address_space();
    
    // Get the objects folder node id
    let objects_folder = NodeId::objects_folder();
    
    // Iterate through applications
    for application in &self.config.application_list {
        // Add application folder
        let app_node_id = NodeId::new(self.ns, format!("Application_{}", application.application_id));
        let app_folder = address_space.add_folder(
            &app_node_id,
            &application.application_name,
            &objects_folder,
        ).await.map_err(|e| {
            OpcGwError::OpcUaError(format!("Failed to add application folder: {}", e))
        })?;
        
        // Add devices for this application
        for device in &application.device_list {
            // Add device folder
            let device_node_id = NodeId::new(self.ns, format!("Device_{}", device.device_id));
            let device_folder = address_space.add_folder(
                &device_node_id,
                &device.device_name,
                &app_folder,
            ).await.map_err(|e| {
                OpcGwError::OpcUaError(format!("Failed to add device folder: {}", e))
            })?;
            
            // Add variables for this device
            self.add_device_variables(&address_space, &device_folder, device).await?;
        }
    }
    
    Ok(())
}
```

#### 2.7. Add Device Variables Method

Create a new method to add device variables:

```rust
async fn add_device_variables(
    &self, 
    address_space: &AddressSpace, 
    parent_node: &NodeId, 
    device: &ChirpstackDevice
) -> Result<(), OpcGwError> {
    for metric in &device.metric_list {
        let node_id = NodeId::new(self.ns, format!("{}_{}", device.device_id, metric.metric_name));
        
        // Create a variable with an initial value
        let initial_value = match metric.metric_type {
            crate::config::OpcMetricTypeConfig::Bool => Variant::Boolean(false),
            crate::config::OpcMetricTypeConfig::Int => Variant::Int32(0),
            crate::config::OpcMetricTypeConfig::Float => Variant::Float(0.0),
            crate::config::OpcMetricTypeConfig::String => Variant::String("".to_string()),
        };
        
        // Add the variable to the address space
        address_space.add_variable(
            &node_id,
            &metric.metric_name,
            parent_node,
            initial_value,
        ).await.map_err(|e| {
            OpcGwError::OpcUaError(format!("Failed to add variable: {}", e))
        })?;
        
        // Set up a data source for this variable
        let device_id = device.device_id.clone();
        let metric_name = metric.chirpstack_metric_name.clone();
        let storage = self.storage.clone();
        
        // Create a data source that will fetch values from storage
        let data_source = move || {
            let storage_guard = storage.lock().expect("Failed to lock storage");
            let value = match storage_guard.get_metric_value(&device_id, &metric_name) {
                Some(MetricType::Float(v)) => Variant::Float(v as f32),
                Some(MetricType::Int(v)) => Variant::Int32(v as i32),
                Some(MetricType::Bool(v)) => Variant::Boolean(v),
                Some(MetricType::String(v)) => Variant::String(v),
                None => Variant::Float(0.0),
            };
            
            DataValue::new(value)
        };
        
        // Register the data source with the variable
        address_space.set_variable_value_getter(&node_id, Box::new(data_source))
            .await
            .map_err(|e| {
                OpcGwError::OpcUaError(format!("Failed to set variable data source: {}", e))
            })?;
    }
    
    Ok(())
}
```

### 3. Simplified Error Handling

For initial migration, keep error handling simple:

```rust
#[error("OPC UA error: {0}")]
OpcUaError(String),
```

Usage example:
```rust
address_space.add_folder(...)
    .await
    .map_err(|e| OpcGwError::OpcUaError(format!("Folder creation failed: {}", e)))?;
```

2. Update custom error types in `src/utils.rs`:
```rust
#[error("OPC UA error: {0}")]
OpcUaError(String),

// Consider adding more specific variants:
#[error("OPC UA connection error: {0}")]
OpcUaConnectionError(String),

#[error("OPC UA address space error: {0}")]  
OpcUaAddressSpaceError(String),
```

3. Wrap async-opcua errors in our custom error type:
```rust
.map_err(|e| OpcUaError(format!("OPC UA operation failed: {}", e)))
```

### 4. Update Main Function

The main function in `src/main.rs` doesn't need significant changes since it's already using async/await and Tokio:

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ... existing code ...

    // Create OPC UA server
    trace!("Create OPC UA server");
    let opc_ua = OpcUa::new(&application_config, storage.clone());

    // Run chirpstack poller and OPC UA server in separate tasks
    let chirpstack_handle = tokio::spawn(async move {
        if let Err(e) = chirpstack_poller.run().await {
            error!("ChirpStack poller error: {:?}", e);
        }
    });

    // Run OPC UA server and periodic metrics reading in separate tasks
    let opcua_handle = tokio::spawn(async move {
        if let Err(e) = opc_ua.run().await {
            error!("OPC UA server error: {:?}", e);
        }
    });

    // Wait for all tasks to complete
    tokio::try_join!(chirpstack_handle, opcua_handle).expect("Failed to run tasks");

    // ... existing code ...
}
```

### 5. Testing the Migration

After implementing these changes, thoroughly test:

1. **Basic Functionality**:
   - Server starts successfully
   - Address space is populated correctly
   - Variables update from storage

2. **Client Connections**:
   - Multiple clients can connect simultaneously  
   - Variable reads return correct values
   - Namespace handling works as expected

3. **Error Cases**:
   - Invalid variable access
   - Server restart scenarios
   - Network interruptions

4. **Performance**:
   - Benchmark with 10+ concurrent clients
   - Measure variable update latency

5. **Automated Tests**:
```rust
#[tokio::test]
async fn test_server_startup() {
    let server = create_test_server();
    assert!(server.run().await.is_ok());
}

#[tokio::test] 
async fn test_variable_updates() {
    let server = create_test_server();
    let value = server.get_variable("test_var").await;
    assert_eq!(value, 0.0);
    
    server.update_variable("test_var", 42.0).await;
    let value = server.get_variable("test_var").await;
    assert_eq!(value, 42.0);
}
```

1. The OPC UA server starts successfully
2. The address space is populated correctly
3. Variables are updated with values from the storage
4. Clients can connect to the server and read variables

### 5. Commit Changes

Once the changes are working, commit them to the `async-opcua` branch:

```bash
# Add all changed files
git add .

# Commit the changes
git commit -m "Migrate from opcua to async-opcua"

# Push the branch to the remote repository
git push -u origin async-opcua
```

### 6. Rollback Plan

If issues are found during testing:

1. **Immediate Rollback Steps**:
```bash
# Revert to main branch
git checkout main

# Delete migration branch
git branch -D async-opcua
```

2. **Known Issues Workarounds**:
   - If async performance is problematic, adjust Tokio runtime configuration
   - For variable update delays, implement batching

3. **Partial Migration**:
   - Consider migrating only non-critical components first
   - Use feature flags to toggle between implementations

### 7. Create a Pull Request

When the migration is complete and tested, create a pull request to merge the `async-opcua` branch into the main branch with:

1. Performance comparison metrics
2. Test coverage report
3. Known limitations section

## Future Improvements & Known Limitations

After initial migration:

1. **Configuration Loading** (Priority: High):
```rust
// TODO: Implement config file loading
// Current hardcoded values work but lack flexibility
```

2. **Security** (Priority: Medium):
   - Add TLS support
   - Implement user authentication

3. **Performance Optimizations**:
   - Variable update batching
   - Cached reads for frequent accesses

4. **Known Limitations**:
   - First release lacks some advanced security features  
   - Variable history not yet implemented
   - Maximum 100 concurrent connections (adjustable in config)

5. **Dependency Notes**:
   - Requires Tokio 1.0+
   - Check feature flags:
```toml
async-opcua = { version = "0.14.0", features = ["server", "encryption"] }
```

1. **Configuration Loading**: Implement loading server configuration from a file
2. **Security**: Add proper security configuration
3. **Error Handling**: Improve error handling with more specific error types
4. **Testing**: Add unit and integration tests for the OPC UA server
5. **Documentation**: Update documentation to reflect the new API

## API Differences

Here are some key differences between the `opcua` and `async-opcua` APIs:

| Feature | opcua | async-opcua |
|---------|-------|-------------|
| Server Creation | `Server::new(config)` wrapped in `Arc<RwLock<>>` | `Server::new(config)` |
| Address Space Access | Through read/write locks | Direct method calls |
| Method Calls | Synchronous | Asynchronous with `.await` |
| Variable Value Updates | Through callbacks | Through value getters |
| Error Handling | Custom error types | Standard error types |

## References

- [async-opcua Documentation](https://docs.rs/async-opcua)
- [async-opcua GitHub Repository](https://github.com/locka99/opcua)
- [Tokio Documentation](https://tokio.rs/docs)
