
use crate::config::ChirpstackConfig;
use crate::chirpstack;
use crate::chirpstack::{ApplicationDetail, ChirpstackClient, DeviceListDetail, DeviceDetails};
use log::{debug, error, info, warn};

/// Test function for ChirpStack operations
pub async fn test_chirpstack(chirpstack_client: &mut ChirpstackClient) {

    test_list_application(chirpstack_client).await;
    test_list_devices(chirpstack_client).await;
    test_get_device_metrics(chirpstack_client).await;
}



async fn test_list_devices(chirpstack_client: &mut ChirpstackClient) {
    // Retrieve and display the list of devices for a specific application
    // Note: The application ID is hardcoded here and might need to be parameterized
    match chirpstack_client.list_devices("194f12ab-d0ab-4389-a446-f1b3e7152b07".to_string()).await {
        Ok(devices) => {
            debug!("Print list of devices");
            print_dev_list(&devices);

            // Test get_device_metrics for the first device in the list
            if let Some(first_device) = devices.first() {
                match chirpstack_client.get_device_details(first_device.dev_eui.clone()).await {
                    Ok(metrics) => {
                        debug!("Device metrics: {:?}", metrics);
                    },
                    Err(e) => error!("Error when getting device metrics: {}", e),
                }
            }
        },
        Err(e) => error!("Error when collecting devices: {}", e),
    }
}


async fn test_list_application(chirpstack_client: &mut ChirpstackClient) {
    // Retrieve and display the list of applications
    match chirpstack_client.list_applications().await {
        Ok(applications) => {
            debug!("Print list of applications");
            print_app_list(&applications)
        },
        Err(e) => error!("Error when collecting applications: {}", e),
    }
}

async fn test_get_device_metrics(chirpstack_client: &mut ChirpstackClient) {
    match chirpstack_client.get_device_metrics("a840414bf185f365".to_string(), 1, 100).await {
        Ok(device) => {
            debug!("Device states: {:#?}", device.states);
            debug!("Device metrics: {:#?}", device.metrics)
        },
        Err(e) => error!("Error when getting device data: {}",e),
    }
}


/// Prints the list of applications to the console
///
/// # Arguments
///
/// `list` - The list of applications to print
///
/// # Returns
///
/// .
pub fn print_app_list(list: &Vec<ApplicationDetail>) {
    for app in list {
        println!(
            "ID: {}, Nom: {}, Description: {}",
            app.id, app.name, app.description
        );
    }
}

/// Prints the list of deices to the console
///
/// # Arguments
///
/// `list` - The list of devices to print
///
/// # Returns
///
/// .
pub fn print_dev_list(list: &Vec<DeviceListDetail>) {
    for dev in list {
        println!(
            "euid: {}, Nom: {}, Description: {}",
            dev.dev_eui, dev.name, dev.description
        );
    }
}