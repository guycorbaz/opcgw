// SPDX-License-Identifier: MIT OR Apache-2.0
// (c) [2024] Guy Corbaz

//! OPC UA HistoryRead service support (Story 8-3, FR22).
//!
//! Wraps async-opcua's `SimpleNodeManagerImpl` so that `HistoryRead` requests
//! for opcgw's metric-variable NodeIds route to `StorageBackend::query_metric_history`.
//! All other `InMemoryNodeManagerImpl` methods are forwarded to the inner
//! `SimpleNodeManagerImpl` so the existing read/subscription/write
//! pipeline (Story 8-2 et al.) is preserved unchanged.
//!
//! # Why a wrap (and not a subclass / new manager)
//!
//! async-opcua 0.17.1 has full HistoryRead **service-level** routing — the
//! session dispatch layer decodes `HistoryReadDetails::RawModified` and calls
//! `node_manager.history_read_raw_modified(...)`. The default
//! `InMemoryNodeManagerImpl::history_read_raw_modified` returns
//! `BadHistoryOperationUnsupported` (`memory_mgr_impl.rs:188-196`), and
//! `SimpleNodeManagerImpl` doesn't override it. To plug HistoryRead into
//! opcgw without touching the existing read/subscription path, this module
//! provides a thin wrapper struct that delegates every other
//! `InMemoryNodeManagerImpl` method to a held `SimpleNodeManagerImpl`.
//!
//! # Continuation points
//!
//! Story 8-3 does **not** implement OPC UA `ByteString` continuation points
//! (Part 11 §6.4.4). When a query exceeds `max_history_data_results_per_node`,
//! the per-node status is `Good`, the response is truncated to the cap, and
//! the SCADA client is expected to issue a follow-up `HistoryRead` with
//! `start = last_returned_row.timestamp + 1µs`. See
//! `docs/security.md#historical-data-access` for the manual-paging recipe.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use opcua::server::{
    address_space::AddressSpace,
    diagnostics::NamespaceMetadata,
    node_manager::{
        memory::{
            InMemoryNodeManager, InMemoryNodeManagerBuilder, InMemoryNodeManagerImpl,
            InMemoryNodeManagerImplBuilder, SimpleNodeManagerBuilder, SimpleNodeManagerImpl,
        },
        MethodCall, MonitoredItemRef, MonitoredItemUpdateRef, NodeManagerBuilder,
        ParsedReadValueId, RequestContext, ServerContext, WriteNode,
    },
    CreateMonitoredItem,
};
use opcua::sync::RwLock;
use opcua::types::HistoryData;
use opcua::types::{
    DataValue, DateTime, MonitoringMode, NodeId, ReadRawModifiedDetails, StatusCode,
    TimestampsToReturn, Variant,
};
use tracing::{debug, error, trace};

use crate::storage::{HistoricalMetricRow, MetricType, StorageBackend};

/// Type alias for the wrapped node manager: an `InMemoryNodeManager`
/// parameterised over our HistoryRead-aware impl.
pub type OpcgwHistoryNodeManager = InMemoryNodeManager<OpcgwHistoryNodeManagerImpl>;

/// HistoryRead-aware wrapper around `SimpleNodeManagerImpl`.
///
/// Forwards every `InMemoryNodeManagerImpl` method to `inner` except
/// `history_read_raw_modified`, which queries `StorageBackend::query_metric_history`
/// and writes results to each `HistoryNode` workspace as `HistoryData`
/// extension objects.
pub struct OpcgwHistoryNodeManagerImpl {
    /// The original SimpleNodeManagerImpl that owns the read/write/method
    /// callback registries and runs the sampler. Every non-history method
    /// is delegated to this field.
    inner: SimpleNodeManagerImpl,
    /// Storage backend with the `query_metric_history` method (Story 8-3 AC#1).
    backend: Arc<dyn StorageBackend>,
    /// Reverse-lookup map: `NodeId` (the OPC UA address-space NodeId for a
    /// metric variable) → `(device_id, metric_name)`. Built once at
    /// server-construction time from the same registration data used for
    /// `add_read_callback`. Immutable for the server's lifetime — Story 8-3
    /// does not implement Epic 9 hot-reload.
    node_to_metric: Arc<RwLock<HashMap<NodeId, (String, String)>>>,
    /// Per-call cap on the number of `HistoryData` rows returned for one
    /// NodeId — `[opcua].max_history_data_results_per_node` (AC#3).
    max_results_per_node: usize,
}

impl OpcgwHistoryNodeManagerImpl {
    /// Re-expose the inner `SimpleNodeManagerImpl` so the existing
    /// `add_read_callback` / `add_write_callback` / `add_method_callback`
    /// inherent methods (used during address-space setup in
    /// `OpcUa::add_nodes`) can be invoked through the wrapper.
    pub fn simple(&self) -> &SimpleNodeManagerImpl {
        &self.inner
    }

}

#[async_trait]
impl InMemoryNodeManagerImpl for OpcgwHistoryNodeManagerImpl {
    async fn init(&self, address_space: &mut AddressSpace, context: ServerContext) {
        self.inner.init(address_space, context).await
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn namespaces(&self) -> Vec<NamespaceMetadata> {
        self.inner.namespaces()
    }

    async fn read_values(
        &self,
        context: &RequestContext,
        address_space: &RwLock<AddressSpace>,
        nodes: &[&ParsedReadValueId],
        max_age: f64,
        timestamps_to_return: TimestampsToReturn,
    ) -> Vec<DataValue> {
        self.inner
            .read_values(context, address_space, nodes, max_age, timestamps_to_return)
            .await
    }

    async fn create_value_monitored_items(
        &self,
        context: &RequestContext,
        address_space: &RwLock<AddressSpace>,
        items: &mut [&mut &mut CreateMonitoredItem],
    ) {
        self.inner
            .create_value_monitored_items(context, address_space, items)
            .await
    }

    async fn modify_monitored_items(
        &self,
        context: &RequestContext,
        items: &[&MonitoredItemUpdateRef],
    ) {
        self.inner.modify_monitored_items(context, items).await
    }

    async fn set_monitoring_mode(
        &self,
        context: &RequestContext,
        mode: MonitoringMode,
        items: &[&MonitoredItemRef],
    ) {
        self.inner.set_monitoring_mode(context, mode, items).await
    }

    async fn delete_monitored_items(
        &self,
        context: &RequestContext,
        items: &[&MonitoredItemRef],
    ) {
        self.inner.delete_monitored_items(context, items).await
    }

    async fn write(
        &self,
        context: &RequestContext,
        address_space: &RwLock<AddressSpace>,
        nodes_to_write: &mut [&mut WriteNode],
    ) -> Result<(), StatusCode> {
        self.inner.write(context, address_space, nodes_to_write).await
    }

    async fn call(
        &self,
        context: &RequestContext,
        address_space: &RwLock<AddressSpace>,
        methods_to_call: &mut [&mut &mut MethodCall],
    ) -> Result<(), StatusCode> {
        self.inner
            .call(context, address_space, methods_to_call)
            .await
    }

    /// **Story 8-3 AC#2 override.** Resolve each requested `NodeId` to a
    /// `(device_id, metric_name)` pair via `node_to_metric`, query
    /// `metric_history` via `StorageBackend::query_metric_history`, and
    /// write results back to each `HistoryNode` as `HistoryData`.
    async fn history_read_raw_modified(
        &self,
        _context: &RequestContext,
        details: &ReadRawModifiedDetails,
        nodes: &mut [&mut &mut opcua::server::node_manager::HistoryNode],
        _timestamps_to_return: TimestampsToReturn,
    ) -> Result<(), StatusCode> {
        // Decode the requested time range. `start_time` and `end_time` are
        // OPC UA `DateTime` values — convert to `SystemTime` for the storage
        // method. AC#1's half-open interval semantics (start inclusive, end
        // exclusive) match Part 11 §6.4 — so we pass through unchanged.
        let start_st: std::time::SystemTime =
            std::time::SystemTime::from(details.start_time.as_chrono());
        let end_st: std::time::SystemTime =
            std::time::SystemTime::from(details.end_time.as_chrono());

        // AC#2 verification: an inverted time range returns
        // `BadInvalidArgument` per OPC UA Part 11 §6.4.2 (server-side
        // validation of range monotonicity).
        if details.end_time != DateTime::null()
            && details.start_time != DateTime::null()
            && end_st < start_st
        {
            for node in nodes.iter_mut() {
                node.set_status(StatusCode::BadInvalidArgument);
            }
            return Ok(());
        }

        // AC#3: the per-NodeId cap. `num_values_per_node` of 0 in the
        // HistoryRead request means "no client-side cap"; honour the
        // server-side default in that case.
        let server_cap = self.max_results_per_node;
        let client_cap = details.num_values_per_node as usize;
        let max_results = if client_cap == 0 {
            server_cap
        } else {
            std::cmp::min(client_cap, server_cap)
        };

        let map = self.node_to_metric.read();

        for node in nodes.iter_mut() {
            let node_id = node.node_id().clone();

            let Some((device_id, metric_name)) = map.get(&node_id) else {
                trace!(
                    node_id = %node_id,
                    "history_read_raw_modified: NodeId not registered for HistoryRead"
                );
                node.set_status(StatusCode::BadNodeIdUnknown);
                continue;
            };

            match self.backend.query_metric_history(
                device_id,
                metric_name,
                start_st,
                end_st,
                max_results,
            ) {
                Ok(rows) => {
                    let data_values = build_data_values(&rows);
                    let history_data = HistoryData {
                        data_values: Some(data_values),
                    };
                    node.set_result(history_data);
                    node.set_status(StatusCode::Good);
                    debug!(
                        node_id = %node_id,
                        device_id = %device_id,
                        metric_name = %metric_name,
                        row_count = rows.len(),
                        "history_read_raw_modified: returning rows"
                    );
                }
                Err(e) => {
                    error!(
                        node_id = %node_id,
                        device_id = %device_id,
                        metric_name = %metric_name,
                        error = %e,
                        "history_read_raw_modified: storage query failed"
                    );
                    // BadHistoryOperationInvalid is the closest async-opcua
                    // 0.17.1 surfaces for storage-side failures (the spec's
                    // BadHistoryOperationFailed isn't in this build of the
                    // status_code table — see async-opcua-types-0.17.1
                    // status_code.rs:697-698).
                    node.set_status(StatusCode::BadHistoryOperationInvalid);
                }
            }
        }

        Ok(())
    }
}

/// Convert a slice of `HistoricalMetricRow` to a `Vec<DataValue>` suitable
/// for OPC UA `HistoryData.data_values`. Rows whose typed parse fails
/// (e.g. Bool with garbage value) are skipped with a `trace!` log per
/// AC#1's partial-success contract.
fn build_data_values(rows: &[HistoricalMetricRow]) -> Vec<DataValue> {
    let mut out = Vec::with_capacity(rows.len());
    let now = DateTime::now();
    for row in rows {
        let variant = match row.data_type {
            MetricType::Float => match row.value.parse::<f64>() {
                Ok(f) if f.is_finite() => Variant::Double(f),
                _ => {
                    trace!(value = %row.value, "history: skipping unparseable Float row");
                    continue;
                }
            },
            MetricType::Int => match row.value.parse::<i64>() {
                Ok(i) => Variant::Int64(i),
                Err(_) => {
                    trace!(value = %row.value, "history: skipping unparseable Int row");
                    continue;
                }
            },
            MetricType::Bool => match bool::from_str(&row.value) {
                Ok(b) => Variant::Boolean(b),
                Err(_) => {
                    trace!(value = %row.value, "history: skipping unparseable Bool row");
                    continue;
                }
            },
            MetricType::String => Variant::String(row.value.clone().into()),
        };
        let dt = DateTime::from(chrono::DateTime::<Utc>::from(row.timestamp));
        out.push(DataValue {
            value: Some(variant),
            status: Some(StatusCode::Good.bits().into()),
            source_timestamp: Some(dt),
            source_picoseconds: None,
            server_timestamp: Some(now),
            server_picoseconds: None,
        });
    }
    out
}

/// Builder for `OpcgwHistoryNodeManagerImpl` — wraps `SimpleNodeManagerBuilder`
/// so the same setup pipeline (namespace registration, NodeSetImport, etc.)
/// runs unchanged.
pub struct OpcgwHistoryNodeManagerBuilder {
    simple: SimpleNodeManagerBuilder,
    backend: Arc<dyn StorageBackend>,
    node_to_metric: Arc<RwLock<HashMap<NodeId, (String, String)>>>,
    max_results_per_node: usize,
}

impl OpcgwHistoryNodeManagerBuilder {
    pub fn new(
        namespace: NamespaceMetadata,
        name: &str,
        backend: Arc<dyn StorageBackend>,
        node_to_metric: Arc<RwLock<HashMap<NodeId, (String, String)>>>,
        max_results_per_node: usize,
    ) -> Self {
        Self {
            simple: SimpleNodeManagerBuilder::new(namespace, name),
            backend,
            node_to_metric,
            max_results_per_node,
        }
    }
}

impl InMemoryNodeManagerImplBuilder for OpcgwHistoryNodeManagerBuilder {
    type Impl = OpcgwHistoryNodeManagerImpl;

    fn build(self, context: ServerContext, address_space: &mut AddressSpace) -> Self::Impl {
        let inner = self.simple.build(context, address_space);
        OpcgwHistoryNodeManagerImpl {
            inner,
            backend: self.backend,
            node_to_metric: self.node_to_metric,
            max_results_per_node: self.max_results_per_node,
        }
    }
}

/// Factory function for the HistoryRead-aware node manager. Mirrors the
/// `simple_node_manager` factory in async-opcua's
/// `node_manager::memory::simple` (see `simple.rs:99`).
pub fn opcgw_history_node_manager(
    namespace: NamespaceMetadata,
    name: &str,
    backend: Arc<dyn StorageBackend>,
    node_to_metric: Arc<RwLock<HashMap<NodeId, (String, String)>>>,
    max_results_per_node: usize,
) -> impl NodeManagerBuilder {
    InMemoryNodeManagerBuilder::new(OpcgwHistoryNodeManagerBuilder::new(
        namespace,
        name,
        backend,
        node_to_metric,
        max_results_per_node,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::memory::InMemoryBackend;
    use crate::storage::MetricType;

    /// Story 8-3 AC#1: `build_data_values` round-trips Float rows from
    /// `HistoricalMetricRow` to `DataValue` with `Variant::Double`.
    #[test]
    fn test_build_data_values_float_round_trip() {
        let rows = vec![HistoricalMetricRow {
            value: "20.5".to_string(),
            data_type: MetricType::Float,
            timestamp: std::time::SystemTime::UNIX_EPOCH
                + std::time::Duration::from_secs(1_700_000_000),
        }];
        let dvs = build_data_values(&rows);
        assert_eq!(dvs.len(), 1);
        match dvs[0].value.as_ref().expect("variant") {
            Variant::Double(f) => assert!((f - 20.5).abs() < 1e-9),
            other => panic!("expected Double, got {:?}", other),
        }
    }

    /// AC#1: Bool values round-trip through `Variant::Boolean`.
    #[test]
    fn test_build_data_values_bool_round_trip() {
        let rows = vec![
            HistoricalMetricRow {
                value: "true".to_string(),
                data_type: MetricType::Bool,
                timestamp: std::time::SystemTime::UNIX_EPOCH,
            },
            HistoricalMetricRow {
                value: "false".to_string(),
                data_type: MetricType::Bool,
                timestamp: std::time::SystemTime::UNIX_EPOCH,
            },
        ];
        let dvs = build_data_values(&rows);
        assert_eq!(dvs.len(), 2);
        assert!(matches!(dvs[0].value, Some(Variant::Boolean(true))));
        assert!(matches!(dvs[1].value, Some(Variant::Boolean(false))));
    }

    /// AC#1 partial-success: rows whose typed parse fails are skipped, not
    /// errored. A garbage Bool value drops out of the result silently.
    #[test]
    fn test_build_data_values_skips_unparseable_bool() {
        let rows = vec![
            HistoricalMetricRow {
                value: "true".to_string(),
                data_type: MetricType::Bool,
                timestamp: std::time::SystemTime::UNIX_EPOCH,
            },
            HistoricalMetricRow {
                value: "garbage".to_string(),
                data_type: MetricType::Bool,
                timestamp: std::time::SystemTime::UNIX_EPOCH,
            },
            HistoricalMetricRow {
                value: "false".to_string(),
                data_type: MetricType::Bool,
                timestamp: std::time::SystemTime::UNIX_EPOCH,
            },
        ];
        let dvs = build_data_values(&rows);
        assert_eq!(dvs.len(), 2, "garbage Bool row must be skipped");
    }

    /// AC#2 sanity: the wrapper exposes the InMemoryBackend (which returns
    /// empty histories) without panicking. This is a smoke-test for the
    /// constructor / field layout.
    #[test]
    fn test_opcgw_history_node_manager_impl_construct() {
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let node_to_metric = Arc::new(RwLock::new(HashMap::new()));
        // The Builder's `build` requires a ServerContext + AddressSpace
        // which are async-opcua-internal types. We only test that the
        // factory function returns a NodeManagerBuilder without panicking.
        let _builder = opcgw_history_node_manager(
            NamespaceMetadata {
                namespace_uri: "urn:opcgw:history:test".to_owned(),
                ..Default::default()
            },
            "test",
            backend,
            node_to_metric,
            10_000,
        );
    }
}
