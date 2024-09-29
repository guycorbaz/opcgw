//! Module pour le stockage des données.
//! 
//! Ce module gère le stockage en mémoire des métriques et la file d'attente pour les commandes.

use std::collections::HashMap;
use tokio::sync::mpsc;

pub struct Storage {
    metrics: HashMap<String, String>,
    command_queue: mpsc::Sender<String>,
}

impl Storage {
    pub fn new() -> (Self, mpsc::Receiver<String>) {
        let (tx, rx) = mpsc::channel(100);
        (
            Storage {
                metrics: HashMap::new(),
                command_queue: tx,
            },
            rx,
        )
    }

    pub fn store_metric(&mut self, key: String, value: String) {
        self.metrics.insert(key, value);
    }

    pub fn get_metric(&self, key: &str) -> Option<&String> {
        self.metrics.get(key)
    }

    pub fn send_command(&self, command: String) -> Result<(), mpsc::error::SendError<String>> {
        self.command_queue.try_send(command)
    }
}
