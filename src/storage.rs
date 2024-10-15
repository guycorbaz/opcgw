// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Manage storage
//!
//! Provide storage management for opc_ua_chirpstack_gateway
//!
//! # Example:
//! Add example code...

use crate::chirpstack::{ApplicationDetail, ChirpstackPoller, DeviceDetails, DeviceListDetail};
use crate::Config;
use log::{debug, error, info, trace, warn};
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Represents a device in the system.
pub struct Device {
    /// Unique identifier of the device.
    pub id: String,
    /// ID of the application this device belongs to.
    pub application_id: String,
    /// Name of the application this device belongs to.
    pub application_name: String,
    /// Name of the device.
    pub name: String,
}

/// Represents an application in the system.
pub struct Application {
    /// Unique identifier of the application.
    pub id: String,
    /// Name of the application.
    pub name: String,
}
/// Main structure for storing application data, metrics, and managing devices and applications.
pub struct Storage {
    config: Config,
    /// Mapping of metric names to their respective values.
    metrics: HashMap<String, String>,
    /// List of applications with their unique identifiers as keys.
    applications: Vec<Application>,
    /// List of devices with their unique identifiers as keys.
    devices: Vec<Device>,

}

impl Storage {
    /// Creates and returns a new instance of `Storage`
    pub async fn new(app_config: &Config) -> Storage {
        // Log a debug message indicating creation of a new Storage instance
        debug!("Creating a new Storage instance");

        Storage {
            config: app_config.clone(),
            metrics: HashMap::new(),
            applications: Vec::new(),
            devices: Vec::new(),
        }
    }

    /// Loads the list of applications from the configuration into the storage.
    pub fn load_applications(&mut self) {
        debug!("Loading applications list");
        for application in &self.config.applications {
            println!("Application {}", application.0.clone());
            let app = Application {
                name: application.0.clone(),
                id: application.1.clone(),
            };
            self.applications.push(app);
        }
    }

    /// Prints the list of applications to the console.
    pub fn list_applications(&self) {
        debug!("Listing applications");
        for application in &self.applications {
            trace!("Application: {:?}", application.name);
        }
    }

    /// Asynchronously loads the list of devices from the configuration and ChirpStack into the storage.
    pub async fn load_devices(&mut self) {
        debug!("Loading devices list");
        todo!();
        //for device in &self.config.devices {
        //    let dev_details = self
        //        .chirpstack_client
        //        .get_device_details(device.1.clone())
        //        .await
        //        .unwrap();
        //    let dev = Device {
        //        id: device.1.clone(),
        //        name: device.0.clone(),
        //        application_id: dev_details.application_id.clone(),
        //        application_name: self
        //            .find_application_name(&dev_details.application_id)
        //            .clone(),
        //    };
        //    self.devices.push(dev);
        //}
    }

    /// Prints the list of devices to the console.
    pub fn list_devices(&self) {
        debug!("Listing devices");
        for device in &self.devices {
            println!(
                "Device {:#?}, linked application: {}",
                device.name, device.application_name
            );
        }
    }

    /// Retrieves the application name for a given application ID.
    fn find_application_name(&self, id: &String) -> String {
        for app in self.applications.iter() {
            if app.id == *id {
                return app.name.clone();
            }
        }
        "".to_string()
    }

    /// Stores a metric with the given key and value.
    pub fn store_metric(&mut self, key: String, value: String) {
        debug!("Storing metric: {} = {}", key, value);
        self.metrics.insert(key, value);
    }

    /// Retrieves a metric value for the given key.
    pub fn get_metric(&self, key: &str) -> Option<&String> {
        debug!("Getting metric: {}", key);
        self.metrics.get(key)
    }

    //pub fn send_command(&self, command: String) -> Result<(), tokio::sync::mpsc::error::SendError<String>> {
    //    self.command_queue.try_send(command)
    //}
}
