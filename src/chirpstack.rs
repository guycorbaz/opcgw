//! Module pour la communication avec ChirpStack.
//! 
//! Ce module gère la connexion et l'interaction avec le serveur ChirpStack.

use crate::config::ChirpstackConfig;

pub struct ChirpstackClient {
    config: ChirpstackConfig,
}

impl ChirpstackClient {
    pub fn new(config: ChirpstackConfig) -> Self {
        ChirpstackClient { config }
    }

    // Ajoutez ici d'autres méthodes pour interagir avec ChirpStack
}
