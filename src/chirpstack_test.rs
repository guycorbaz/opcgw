
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

    pub async fn get_device_metrics(&self, dev_eui: String) -> Result<DeviceMetrics, AppError> {
        debug!("Get device metrics for DevEUI: {}", dev_eui);
        let request = Request::new(GetDeviceRequest {
            dev_eui,
        });

        match self.device_client.get(request).await {
            Ok(response) => {
                let device = response.into_inner();
                Ok(DeviceMetrics {
                    dev_eui: device.dev_eui,
                    battery_level: device.device_status.battery_level,
                    margin: device.device_status.margin,
                    // Ajoutez d'autres métriques selon vos besoins
                })
            },
            Err(e) => Err(AppError::ChirpStackError(format!("Failed to get device metrics: {}", e))),
        }
    }
}

#[derive(Debug)]
pub struct DeviceMetrics {
    pub dev_eui: String,
    pub battery_level: u32,
    pub margin: i32,
    // Ajoutez d'autres champs de métriques selon vos besoins
}
