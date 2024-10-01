
use crate::config::ChirpstackConfig;
use crate::chirpstack;
use crate::chirpstack::ChirpstackClient;
use log::{debug, error, info, warn};

/// Test function for ChirpStack operations
pub async fn test_chirpstack(chirpstack_client: ChirpstackClient) {
    // Retrieve and display the list of applications
    match chirpstack_client.list_applications().await {
        Ok(applications) => {
            debug!("Print list of applications");
            chirpstack::print_app_list(&applications)
        },
        Err(e) => error!("Error when collecting applications: {}", e),
    }

    // Retrieve and display the list of devices for a specific application
    // Note: The application ID is hardcoded here and might need to be parameterized
    match chirpstack_client.list_devices("194f12ab-d0ab-4389-a446-f1b3e7152b07".to_string()).await {
        Ok(devices) => {
            debug!("Print list of devices");
            chirpstack::print_dev_list(&devices);
        },
        Err(e) => error!("Error when collecting devices: {}", e),
    }
}
