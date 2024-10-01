

use crate::chirpstack;
use crate::chirpstack::ChirpstackClient;
use log::{debug, error, info, warn};


pub async fn test_chirpstack( chirpstack_client: ChirpstackClient) {


    // Get the list of applications TODO: remove after testing
    match chirpstack_client.list_applications("52f14cd4-c6f1-4fbd-8f87-4025e1d49242".to_string()).await {
        Ok(applications) => {
            debug!("Print list of applications");
            chirpstack::print_app_list(&applications)
        },
        Err(e) => error!("Error when collecting applications: {}",e),
    }

    // Get the list of devices TODO: remove after testing
    match chirpstack_client.list_devices("ae2012c2-75a1-407d-98ab-1520fb511edf".to_string()).await {
        Ok(devices) => {
            debug!("Print list of devices");
            chirpstack::print_dev_list(&devices);
        },
        Err(e) => error!("Error when collecting devices: {}",e),
    }
}