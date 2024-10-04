//! Module pour le stockage des données.
//!
//! Ce module gère le stockage en mémoire des métriques et la file d'attente pour les commandes.

use log::{debug, error, info, trace, warn};
use std::collections::HashMap;
use tokio::sync::mpsc;
use crate::chirpstack::{ApplicationDetail, ChirpstackClient, DeviceDetails, DeviceListDetail};
use crate::Config;

pub struct Storage {
    metrics: HashMap<String, String>,
    applications: HashMap<String, String>,
    devices: HashMap<String, String>,
}

impl Storage {
    //pub fn new() -> (Self, mpsc::Receiver<String>) {
    pub fn new()  -> Storage {
        debug!("Create a new Storage");
        //let (tx, rx) = mpsc::channel(100);

            Storage {
                metrics: HashMap::new(),
                //command_queue: tx,
                applications: HashMap::new(),
                devices: HashMap::new(),
            }
    }


    pub fn load_applications_list(&mut self, applications_list: &Vec<ApplicationDetail>) {
        debug!("create_applications");
        trace!("applications_list: {:?}", applications_list);
        for application in applications_list {
            self.applications.insert(
                application.name.clone(),
                application.id.clone(),
            );
        }
    }

    pub fn print_applications_list(&self) {
        debug!("List applications");
        for application in &self.applications {
            trace!("Application: {:?}", application);
        }
    }

    pub fn load_devices_list(&mut self) {
        debug!("create_devices");
    }

    pub fn print_devices_list(&self) {
        debug!("List devices");
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
