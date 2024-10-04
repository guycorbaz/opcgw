//! Module pour le stockage des données.
//!
//! Ce module gère le stockage en mémoire des métriques et la file d'attente pour les commandes.

use log::{debug, error, info, trace, warn};
use std::collections::HashMap;
use tokio::sync::mpsc;
use crate::chirpstack::{ApplicationDetail, ChirpstackClient, DeviceDetails, DeviceListDetail};
use crate::Config;

pub struct Device {
    pub id: String,
    pub application_id: String,
    pub application_name: String,
    pub name: String,
}

pub struct Application {
    pub id: String,
    pub name: String,
}
/// Structure for storing application data.
pub struct Storage {
    config: Config,
    /// Mapping of metric names to their respective values.
    metrics: HashMap<String, String>,

    /// List of applications with their unique identifiers as keys.
    applications: Vec<Application>,

    /// List of devices with their unique identifiers as keys.
    devices: Vec<Device>,

    /// Instance of the Chirpstack client for interacting with the Chirpstack server.
    chirpstack_client: ChirpstackClient,
}


impl Storage {
    /// Creates and returns a new instance of `Storage`
    ///
    /// # Arguments
    ///
    /// * `app_config` - A reference to the `Config` structure which holds the application configurations
    ///
    /// # Returns
    ///
    /// * `Storage` - An instance of the `Storage` structure initialized with default values
    ///
    /// # Example
    ///
    /// ```rust
    /// let config = Config::new();
    /// let storage = new(&config).await;
    /// ```
    pub async fn new(app_config: &Config) -> Storage {
        // Log a debug message indicating creation of a new Storage instance
        debug!("Create a new Storage");


        Storage {
            config: app_config.clone(),
            metrics: HashMap::new(),
            applications: Vec::new(),
            devices: Vec::new(),
            chirpstack_client: ChirpstackClient::new(&app_config.chirpstack.clone()).await.unwrap(),
        }
    }



    pub fn load_applications_list(&mut self) {
        debug!("create_applications");
        for application in &self.config.applications {
            println!("Application {}", application.0.clone());
            let app = Application {
                name: application.0.clone(),
                id: application.1.clone(),
            };
            self.applications.push(app);
        }
    }

    pub fn print_applications_list(&self) {
        debug!("List applications");
        for application in &self.applications {
            trace!("Application: {:?}", application.name);
        }
    }


    pub async  fn load_devices_list(&mut self) {
        debug!("create_devices");
        for device in &self.config.devices {
            let dev_details = self.chirpstack_client
                .get_device_details(device.1
                    .clone())
                .await
                .unwrap();

            let dev = Device {
                id: device.1.clone(),
                name: device.0.clone(),
                application_id: dev_details.application_id.clone(),
                application_name: self.get_application_name(&dev_details.application_id).clone(),
            };
            &self.devices.push(dev);
        }
    }

    pub fn print_devices_list(&self) {
        debug!("List devices");
        for device in &self.devices {
            println!("Device {:#?}, linked application: {}", device.name, device.application_name);
        }
    }

    fn get_application_name(&self, id: &String) -> String {
        for app in self.applications.iter() {
            if app.id == *id {
                return app.name.clone();
            }
        }
        "".to_string()
    }

    pub fn store_metric(&mut self, key: String, value: String) {
        debug!("store");
        self.metrics.insert(key, value);
    }

    pub fn get_metric(&self, key: &str) -> Option<&String> {
        debug!("get");
        self.metrics.get(key)
    }

    //pub fn send_command(&self, command: String) -> Result<(), tokio::sync::mpsc::error::SendError<String>> {
    //    self.command_queue.try_send(command)
    //}
}
