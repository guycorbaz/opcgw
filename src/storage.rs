// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Manage storage
//!
//! Provide storage management for opc_ua_chirpstack_gateway
//!
//! # Example:
//! Add example code...

use log::{debug, error, info, trace, warn};
use std::collections::HashMap;
use tokio::sync::mpsc;
use crate::chirpstack::{ApplicationDetail, ChirpstackClient, DeviceDetails, DeviceListDetail};
use crate::Config;

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
        debug!("Creating a new Storage instance");

        Storage {
            config: app_config.clone(),
            metrics: HashMap::new(),
            applications: Vec::new(),
            devices: Vec::new(),
            chirpstack_client: ChirpstackClient::new(&app_config.chirpstack.clone()).await.unwrap(),
        }
    }



    /// Loads the list of applications from the configuration into the storage.
    ///
    /// This function iterates over the list of applications found in the configuration.
    /// For each application, it creates an `Application` instance containing the application's
    /// name and ID, and then it appends this instance to the internal `applications` storage.
    ///
    /// # Examples
    ///
    /// ```rust
    /// // Assuming `self` is an instance of a struct containing
    /// // a `config` field with `applications` list and an `applications` storage.
    ///
    /// self.load_applications_list();
    /// ```
    ///
    /// # Debug Information
    ///
    /// - The function logs a debug message "Loading applications list" at the start.
    /// - It prints each application's name as it's being processed.
    ///
    /// # Panics
    ///
    /// This function does not explicitly panic, but be mindful of any potential panics induced
    /// by `clone` or `push` operations if the underlying implementation changes.
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
    ///
    /// This method iterates over the `applications` field of the struct and
    /// prints the name of each application. The logging is done at the `debug`
    /// and `trace` levels, which means that these messages will only be
    /// shown if the logging is configured to display these levels.
    ///
    /// # Examples
    ///
    /// ```
    /// let my_struct = MyStruct {
    ///     applications: vec![
    ///         Application { name: String::from("App1") },
    ///         Application { name: String::from("App2") },
    ///     ],
    /// };
    ///
    /// my_struct.print_applications_list();
    /// ```
    ///
    /// The above example will print:
    /// ```
    /// Listing applications
    /// Application: "App1"
    /// Application: "App2"
    /// ```
    /// assuming the logging level is set to debug or lower.
    pub fn list_applications(&self) {
        debug!("Listing applications");
        for application in &self.applications {
            trace!("Application: {:?}", application.name);
        }
    }

    /// Asynchronously loads the list of devices from the configuration and ChirpStack into the storage.
    ///
    /// This function performs the following steps:
    /// 1. Logs a debug message indicating the start of the device loading process.
    /// 2. Iterates over the devices specified in the configuration.
    /// 3. Fetches detailed device information from ChirpStack using the device ID.
    /// 4. Constructs a `Device` struct with the fetched details and additional information like
    ///    application name and ID.
    /// 5. Appends the constructed `Device` struct to the list of devices in the current storage.
    ///
    /// # Panics
    /// This function will panic if the `get_device_details` call to ChirpStack fails.
    ///
    /// # Examples
    /// ```rust
    /// async {
    ///     instance.load_devices_list().await;
    /// }
    /// ```
    pub async fn load_devices(&mut self) {
        debug!("Loading devices list");
        for device in &self.config.devices {
            let dev_details = self.chirpstack_client
                .get_device_details(device.1.clone())
                .await
                .unwrap();
            let dev = Device {
                id: device.1.clone(),
                name: device.0.clone(),
                application_id: dev_details.application_id.clone(),
                application_name: self.find_application_name(&dev_details.application_id).clone(),
            };
            self.devices.push(dev);
        }
    }

    /// Prints the list of devices to the console.
    /// Prints the list of devices and their linked applications.
    ///
    /// This function logs a debug message indicating that it is listing devices.
    /// It then iterates over the devices in the `self.devices` vector and prints
    /// each device's name along with the name of the linked application to the console.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let manager = DeviceManager::new();
    /// manager.print_devices_list();
    /// ```
    ///
    /// # Panics
    ///
    /// This function does not panic.
    ///
    /// # Errors
    ///
    /// This function does not return errors.
    ///
    pub fn list_devices(&self) {
        debug!("Listing devices");
        for device in &self.devices {
            println!("Device {:#?}, linked application: {}", device.name, device.application_name);
        }
    }

    /// Retrieves the application name for a given application ID.
    ///
    /// # Arguments
    ///
    /// * `id` - The ID of the application to look up.
    ///
    /// # Returns
    ///
    /// The name of the application if found, or an empty string if not found.
    fn find_application_name(&self, id: &String) -> String {
        for app in self.applications.iter() {
            if app.id == *id {
                return app.name.clone();
            }
        }
        "".to_string()
    }

    /// Stores a metric with the given key and value.
    ///
    /// # Arguments
    ///
    /// * `key` - The key for the metric.
    /// * `value` - The value of the metric.
    pub fn store_metric(&mut self, key: String, value: String) {
        debug!("Storing metric: {} = {}", key, value);
        self.metrics.insert(key, value);
    }

    /// Retrieves a metric value for the given key.
    ///
    /// # Arguments
    ///
    /// * `key` - The key of the metric to retrieve.
    ///
    /// # Returns
    ///
    /// An Option containing a reference to the metric value if found, or None if not found.
    pub fn get_metric(&self, key: &str) -> Option<&String> {
        debug!("Getting metric: {}", key);
        self.metrics.get(key)
    }

    //pub fn send_command(&self, command: String) -> Result<(), tokio::sync::mpsc::error::SendError<String>> {
    //    self.command_queue.try_send(command)
    //}
}
