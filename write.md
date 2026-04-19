# Interface d'écriture OPC UA avec async_opcua

## Vue d'ensemble

Ce document décrit comment ajouter une interface d'écriture pour les variables OPC UA en utilisant async_opcua dans le projet opcgw.

## 1. Ajouter des callbacks d'écriture aux variables

Dans la méthode `add_nodes()` du fichier `src/opc_ua.rs`, après avoir ajouté le callback de lecture, ajouter un callback d'écriture :

```rust
// Dans add_nodes(), après le add_read_callback
let storage_clone_write = self.storage.clone();
let device_id_write = device.device_id.clone();
let chirpstack_metric_name_write = metric.chirpstack_metric_name.clone();

manager
    .inner()
    .add_write_callback(metric_node.clone(), move |_, _, value| {
        Self::set_value(
            &storage_clone_write,
            device_id_write.clone(),
            chirpstack_metric_name_write.clone(),
            value,
        )
    });
```

## 2. Implémenter la méthode `set_value`

Ajouter cette nouvelle méthode dans l'impl de `OpcUa` :

```rust
/// Écrit une valeur dans le storage depuis une variable OPC UA.
///
/// Cette méthode sert de callback pour les opérations d'écriture OPC UA, recevant
/// une valeur depuis un client OPC UA et la stockant dans le système de storage
/// partagé après conversion de type appropriée.
///
/// # Flux de données
///
/// 1. **Accès au Storage** : Acquiert un verrou sur le storage partagé
/// 2. **Validation de la valeur** : Vérifie la présence d'une valeur dans DataValue
/// 3. **Conversion de type** : Convertit le Variant OPC UA vers MetricType interne
/// 4. **Stockage** : Sauvegarde la valeur convertie dans le storage
///
/// # Arguments
///
/// * `storage` - Référence thread-safe vers le storage contenant les valeurs métriques
/// * `device_id` - Identifiant unique du dispositif dont la métrique est écrite
/// * `metric_name` - Nom de la métrique spécifique à modifier
/// * `data_value` - Valeur OPC UA à écrire contenant le variant et les métadonnées
///
/// # Retours
///
/// * `Ok(())` - Valeur écrite avec succès dans le storage
/// * `Err(StatusCode)` - Conditions d'erreur :
///   - `BadTypeMismatch` - Échec de conversion de type
///   - `BadDataUnavailable` - Aucune valeur fournie
///   - `BadInternalError` - Échec d'acquisition du verrou storage
fn set_value(
    storage: &Arc<std::sync::Mutex<Storage>>,
    device_id: String,
    metric_name: String,
    data_value: &DataValue,
) -> Result<(), opcua::types::StatusCode> {
    trace!("Set value for device {} and metric {}", device_id, metric_name);

    match storage.lock() {
        Ok(mut storage_guard) => {
            if let Some(variant) = &data_value.value {
                match Self::convert_variant_to_metric(variant) {
                    Ok(metric_value) => {
                        storage_guard.set_metric_value(&device_id, &metric_name, metric_value);
                        Ok(())
                    }
                    Err(e) => {
                        error!("Failed to convert variant to metric: {}", e);
                        Err(opcua::types::StatusCode::BadTypeMismatch)
                    }
                }
            } else {
                error!("No value provided for write operation");
                Err(opcua::types::StatusCode::BadDataUnavailable)
            }
        }
        Err(e) => {
            error!("Impossible to lock storage for write: {}", e);
            Err(opcua::types::StatusCode::BadInternalError)
        }
    }
}
```

## 3. Ajouter la conversion inverse Variant → MetricType

```rust
/// Convertit un Variant OPC UA vers le type métrique interne.
///
/// Cette méthode effectue la conversion inverse de `convert_metric_to_variant`,
/// transformant les types OPC UA Variant vers l'énumération `MetricType` interne
/// de l'application pour le stockage des données.
///
/// # Mappages de types
///
/// | Type OPC UA Variant | Type MetricType interne | Notes |
/// |---------------------|-------------------------|--------|
/// | `Variant::Int32` | `MetricType::Int` | Converti vers i64 |
/// | `Variant::Int64` | `MetricType::Int` | Conversion directe |
/// | `Variant::Float` | `MetricType::Float` | Converti vers f64 |
/// | `Variant::Double` | `MetricType::Float` | Conversion directe |
/// | `Variant::String` | `MetricType::String` | Conversion directe |
/// | `Variant::Boolean` | `MetricType::Bool` | Conversion directe |
///
/// # Arguments
///
/// * `variant` - Le variant OPC UA à convertir
///
/// # Retours
///
/// * `Ok(MetricType)` - Conversion réussie vers le type métrique interne
/// * `Err(String)` - Type de variant non supporté avec message d'erreur
fn convert_variant_to_metric(variant: &Variant) -> Result<MetricType, String> {
    match variant {
        Variant::Int32(value) => Ok(MetricType::Int(*value as i64)),
        Variant::Int64(value) => Ok(MetricType::Int(*value)),
        Variant::Float(value) => Ok(MetricType::Float(*value as f64)),
        Variant::Double(value) => Ok(MetricType::Float(*value)),
        Variant::String(value) => Ok(MetricType::String(value.to_string())),
        Variant::Boolean(value) => Ok(MetricType::Bool(*value)),
        _ => Err(format!("Unsupported variant type: {:?}", variant)),
    }
}
```

## 4. Modifier la création des variables pour autoriser l'écriture

Dans `add_nodes()`, modifier la création des variables pour les rendre accessibles en écriture :

```rust
// Remplacer la création de variable existante par :
let mut variable = Variable::new(
    &metric_node,
    metric.metric_name.clone(),
    metric.metric_name.clone(),
    0_i32,
);

// Rendre la variable accessible en écriture
variable = variable.writable();

let _ = address_space.add_variables(vec![variable], &device_node);
```

## 5. Validation des types (optionnel)

Pour plus de robustesse, ajouter une validation basée sur la configuration :

```rust
/// Valide que le type de variant correspond au type attendu pour la métrique.
///
/// Cette fonction optionnelle vérifie la cohérence entre le type de données
/// reçu via OPC UA et le type configuré pour la métrique dans la configuration
/// de l'application.
fn validate_write_type(
    config: &AppConfig,
    device_id: &str,
    metric_name: &str,
    variant: &Variant,
) -> Result<(), String> {
    if let Some(expected_type) = config.get_metric_type(&metric_name.to_string(), &device_id.to_string()) {
        match (expected_type, variant) {
            (OpcMetricTypeConfig::Float, Variant::Float(_)) |
            (OpcMetricTypeConfig::Float, Variant::Double(_)) => Ok(()),
            (OpcMetricTypeConfig::Int, Variant::Int32(_)) |
            (OpcMetricTypeConfig::Int, Variant::Int64(_)) => Ok(()),
            (OpcMetricTypeConfig::Bool, Variant::Boolean(_)) => Ok(()),
            (OpcMetricTypeConfig::String, Variant::String(_)) => Ok(()),
            _ => Err("Type mismatch".to_string()),
        }
    } else {
        Ok(()) // Pas de validation si type non configuré
    }
}
```

## Résumé des modifications

1. **Callbacks d'écriture** : Ajout de `add_write_callback` pour chaque variable métrique
2. **Méthode `set_value`** : Gestion des écritures OPC UA vers le storage
3. **Conversion inverse** : `convert_variant_to_metric` pour transformer les types OPC UA
4. **Variables accessibles en écriture** : Utilisation de `.writable()` sur les variables
5. **Validation optionnelle** : Vérification de cohérence des types

Cette approche permet aux clients OPC UA d'écrire des valeurs dans les variables, qui seront automatiquement stockées dans le système de storage partagé de l'application.

## Notes d'implémentation

### Sécurité des threads
- Utilisation d'`Arc<Mutex<Storage>>` pour l'accès concurrent sécurisé
- Clonage des données nécessaires pour les closures asynchrones
- Gestion appropriée des verrous pour éviter les deadlocks

### Gestion d'erreurs
- Codes de statut OPC UA appropriés pour chaque type d'erreur
- Logging détaillé pour le débogage
- Validation des types pour éviter les erreurs de conversion

### Performance
- Conversion de types efficace sans copies inutiles
- Accès direct au storage sans couches intermédiaires
- Callbacks légers pour minimiser la latence d'écriture
