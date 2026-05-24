/* SPDX-License-Identifier: MIT OR Apache-2.0
 * (c) [2026] Guy Corbaz
 *
 * Story C-4: client-side drift view.
 *
 * Calls GET /api/inventory/drift and renders the 4-class diff table.
 * Action buttons dispatch through existing CRUD paths (DELETE / PUT) or
 * deep-link to C-2 picker pages with `prefill_*` query parameters
 * (Task 3 handles the picker-side honor). The drift-action audit
 * endpoint records the operator's intent BEFORE any CRUD fires; the
 * actual CRUD emits its own audit event (`application_crud`, etc.).
 *
 * No background polling — refresh is operator-triggered (button +
 * initial page load only).
 */
(function () {
  'use strict';

  const escapeHtml = (s) => {
    if (s === null || s === undefined) return '';
    return String(s)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;');
  };

  const state = {
    chirpstackReachable: true,
    lastResponse: null,
  };

  function setText(id, value) {
    const el = document.getElementById(id);
    if (el) el.textContent = value;
  }

  function show(id) {
    const el = document.getElementById(id);
    if (el) el.hidden = false;
  }

  function hide(id) {
    const el = document.getElementById(id);
    if (el) el.hidden = true;
  }

  async function emitAudit(event, fields) {
    try {
      await fetch('/api/audit/drift-action', {
        method: 'POST',
        credentials: 'include',
        headers: {
          'Content-Type': 'application/json',
        },
        body: JSON.stringify({ event, fields }),
      });
    } catch (e) {
      /* audit is best-effort; the operator's CRUD action still proceeds */
    }
  }

  function summaryPill(klass, count) {
    return (
      '<span class="pill" data-class="' +
      escapeHtml(klass) +
      '">' +
      escapeHtml(klass) +
      ': ' +
      count +
      '</span>'
    );
  }

  function renderSummary(summary) {
    const el = document.getElementById('drift-summary');
    if (!el) return;
    el.innerHTML =
      summaryPill('ok', summary.ok) +
      summaryPill('stale', summary.stale) +
      summaryPill('available', summary.available) +
      summaryPill('drifted', summary.drifted) +
      '<span class="pill">total: ' +
      summary.total +
      '</span>';
    setText('drift-count-ok', summary.ok);
    setText('drift-count-stale', summary.stale);
    setText('drift-count-available', summary.available);
    setText('drift-count-drifted', summary.drifted);
  }

  function clearTbody(tableId) {
    const tbody = document.querySelector('#' + tableId + ' tbody');
    if (tbody) tbody.innerHTML = '';
  }

  function appendRow(tableId, htmlRow) {
    const tbody = document.querySelector('#' + tableId + ' tbody');
    if (!tbody) return;
    const tr = document.createElement('tr');
    tr.innerHTML = htmlRow;
    tbody.appendChild(tr);
  }

  function appendOkRow(scope, resource, value) {
    appendRow(
      'drift-table-ok',
      '<td>' +
        escapeHtml(scope) +
        '</td><td>' +
        escapeHtml(resource) +
        '</td><td>' +
        escapeHtml(value) +
        '</td>'
    );
  }

  function appendDriftedRow(scope, resource, opcgwValue, chirpstackValue, actionsHtml) {
    appendRow(
      'drift-table-drifted',
      '<td>' +
        escapeHtml(scope) +
        '</td><td>' +
        escapeHtml(resource) +
        '</td><td>' +
        escapeHtml(opcgwValue) +
        '</td><td>' +
        escapeHtml(chirpstackValue) +
        '</td><td><div class="drift-row-actions">' +
        actionsHtml +
        '</div></td>'
    );
  }

  function appendStaleRow(scope, resource, opcgwValue, reason, actionsHtml, softStale) {
    const reasonCell = softStale
      ? '<td class="soft-stale">' + escapeHtml(reason) + '</td>'
      : '<td>' + escapeHtml(reason) + '</td>';
    appendRow(
      'drift-table-stale',
      '<td>' +
        escapeHtml(scope) +
        '</td><td>' +
        escapeHtml(resource) +
        '</td><td>' +
        escapeHtml(opcgwValue) +
        '</td>' +
        reasonCell +
        '<td><div class="drift-row-actions">' +
        actionsHtml +
        '</div></td>'
    );
  }

  function appendAvailableRow(scope, resource, chirpstackValue, actionsHtml) {
    appendRow(
      'drift-table-available',
      '<td>' +
        escapeHtml(scope) +
        '</td><td>' +
        escapeHtml(resource) +
        '</td><td>' +
        escapeHtml(chirpstackValue) +
        '</td><td><div class="drift-row-actions">' +
        actionsHtml +
        '</div></td>'
    );
  }

  /* ----- Action button HTML builders. Each button's data-* attributes
   * carry the scope so the click-handler (event delegation) can
   * reconstruct the CRUD endpoint / deep-link / audit payload. */

  function actionsForStaleApplication(appId) {
    const disabled = state.chirpstackReachable ? '' : 'disabled';
    return (
      '<button type="button" class="btn-remove" data-act="remove-app" data-app-id="' +
      escapeHtml(appId) +
      '" ' +
      disabled +
      '>Remove from opcgw</button>' +
      '<button type="button" class="btn-keep" data-act="keep-stale-app" data-app-id="' +
      escapeHtml(appId) +
      '">Keep as alias</button>'
    );
  }

  function actionsForDriftedApplication(appId, newName) {
    const disabled = state.chirpstackReachable ? '' : 'disabled';
    return (
      '<button type="button" class="btn-update" data-act="rename-app" data-app-id="' +
      escapeHtml(appId) +
      '" data-new-name="' +
      escapeHtml(newName) +
      '" ' +
      disabled +
      '>Update opcgw name</button>' +
      '<button type="button" class="btn-keep" data-act="keep-drifted-app" data-app-id="' +
      escapeHtml(appId) +
      '">Keep opcgw alias</button>'
    );
  }

  function actionsForAvailableApplication(csId, csName) {
    if (!state.chirpstackReachable) return '';
    return (
      '<button type="button" class="btn-add" data-act="add-app" data-app-id="' +
      escapeHtml(csId) +
      '" data-app-name="' +
      escapeHtml(csName) +
      '">Add to opcgw</button>'
    );
  }

  function actionsForStaleDevice(appId, devId, reason) {
    const disabled = state.chirpstackReachable ? '' : 'disabled';
    return (
      '<button type="button" class="btn-remove" data-act="remove-dev" data-app-id="' +
      escapeHtml(appId) +
      '" data-dev-id="' +
      escapeHtml(devId) +
      '" ' +
      disabled +
      '>Remove from opcgw</button>' +
      '<button type="button" class="btn-keep" data-act="keep-stale-dev" data-app-id="' +
      escapeHtml(appId) +
      '" data-dev-id="' +
      escapeHtml(devId) +
      '" data-reason="' +
      escapeHtml(reason || '') +
      '">Keep as alias</button>'
    );
  }

  function actionsForDriftedDevice(appId, devId, newName) {
    const disabled = state.chirpstackReachable ? '' : 'disabled';
    return (
      '<button type="button" class="btn-update" data-act="rename-dev" data-app-id="' +
      escapeHtml(appId) +
      '" data-dev-id="' +
      escapeHtml(devId) +
      '" data-new-name="' +
      escapeHtml(newName) +
      '" ' +
      disabled +
      '>Update opcgw name</button>' +
      '<button type="button" class="btn-keep" data-act="keep-drifted-dev" data-app-id="' +
      escapeHtml(appId) +
      '" data-dev-id="' +
      escapeHtml(devId) +
      '">Keep opcgw alias</button>'
    );
  }

  function actionsForAvailableDevice(appId, devEui, devName) {
    if (!state.chirpstackReachable) return '';
    return (
      '<button type="button" class="btn-add" data-act="add-dev" data-app-id="' +
      escapeHtml(appId) +
      '" data-dev-eui="' +
      escapeHtml(devEui) +
      '" data-dev-name="' +
      escapeHtml(devName) +
      '">Add to opcgw</button>'
    );
  }

  function actionsForStaleMetric(appId, devId, metricKey, reason) {
    const disabled = state.chirpstackReachable ? '' : 'disabled';
    return (
      '<button type="button" class="btn-remove" data-act="remove-metric" data-app-id="' +
      escapeHtml(appId) +
      '" data-dev-id="' +
      escapeHtml(devId) +
      '" data-metric-key="' +
      escapeHtml(metricKey) +
      '" ' +
      disabled +
      '>Remove from opcgw</button>' +
      '<button type="button" class="btn-keep" data-act="keep-stale-metric" data-app-id="' +
      escapeHtml(appId) +
      '" data-dev-id="' +
      escapeHtml(devId) +
      '" data-metric-key="' +
      escapeHtml(metricKey) +
      '" data-reason="' +
      escapeHtml(reason || '') +
      '">Keep as alias</button>'
    );
  }

  function actionsForDriftedMetric(appId, devId, metricKey, inferredType) {
    const disabled = state.chirpstackReachable ? '' : 'disabled';
    return (
      '<button type="button" class="btn-update" data-act="update-metric-type" data-app-id="' +
      escapeHtml(appId) +
      '" data-dev-id="' +
      escapeHtml(devId) +
      '" data-metric-key="' +
      escapeHtml(metricKey) +
      '" data-new-type="' +
      escapeHtml(inferredType) +
      '" ' +
      disabled +
      '>Update wire type to inferred</button>' +
      '<button type="button" class="btn-keep" data-act="keep-drifted-metric" data-app-id="' +
      escapeHtml(appId) +
      '" data-dev-id="' +
      escapeHtml(devId) +
      '" data-metric-key="' +
      escapeHtml(metricKey) +
      '">Keep configured type</button>'
    );
  }

  function actionsForAvailableMetric(appId, devId, key) {
    if (!state.chirpstackReachable) return '';
    return (
      '<button type="button" class="btn-add" data-act="add-metric" data-app-id="' +
      escapeHtml(appId) +
      '" data-dev-id="' +
      escapeHtml(devId) +
      '" data-metric-key="' +
      escapeHtml(key) +
      '">Add to opcgw</button>'
    );
  }

  /* ----- Render the full table from the response. */

  function renderRows(resp) {
    clearTbody('drift-table-ok');
    clearTbody('drift-table-stale');
    clearTbody('drift-table-available');
    clearTbody('drift-table-drifted');

    for (const row of resp.applications || []) {
      switch (row.class) {
        case 'ok':
          appendOkRow('Application', row.opcgw.application_id, row.opcgw.application_name);
          break;
        case 'stale':
          appendStaleRow(
            'Application',
            row.opcgw.application_id,
            row.opcgw.application_name,
            'No longer in ChirpStack',
            actionsForStaleApplication(row.opcgw.application_id),
            false
          );
          break;
        case 'available':
          appendAvailableRow(
            'Application',
            row.chirpstack.id,
            row.chirpstack.name,
            actionsForAvailableApplication(row.chirpstack.id, row.chirpstack.name)
          );
          break;
        case 'drifted':
          appendDriftedRow(
            'Application',
            row.opcgw.application_id,
            row.opcgw.application_name,
            row.chirpstack.name,
            actionsForDriftedApplication(row.opcgw.application_id, row.chirpstack.name)
          );
          break;
      }
    }

    for (const row of resp.devices || []) {
      const scope = 'Device · ' + row.application_id;
      switch (row.class) {
        case 'ok':
          appendOkRow(scope, row.opcgw.device_id, row.opcgw.device_name);
          break;
        case 'stale':
          appendStaleRow(
            scope,
            row.opcgw.device_id,
            row.opcgw.device_name,
            'No longer in ChirpStack',
            actionsForStaleDevice(row.application_id, row.opcgw.device_id, 'stale'),
            false
          );
          break;
        case 'available':
          appendAvailableRow(
            scope,
            row.chirpstack.dev_eui,
            row.chirpstack.name,
            actionsForAvailableDevice(row.application_id, row.chirpstack.dev_eui, row.chirpstack.name)
          );
          break;
        case 'drifted':
          appendDriftedRow(
            scope,
            row.opcgw.device_id,
            row.opcgw.device_name,
            row.chirpstack.name,
            actionsForDriftedDevice(row.application_id, row.opcgw.device_id, row.chirpstack.name)
          );
          break;
      }
    }

    for (const row of resp.metrics || []) {
      const scope = 'Metric · ' + row.application_id + ' / ' + row.device_id;
      const reason = row.drift_details ? row.drift_details.reason : '';
      const softStale = reason === 'not_in_recent_uplinks';
      switch (row.class) {
        case 'ok':
          appendOkRow(scope, row.opcgw.chirpstack_metric_name, row.opcgw.metric_type);
          break;
        case 'stale':
          appendStaleRow(
            scope,
            row.opcgw.chirpstack_metric_name,
            row.opcgw.metric_name,
            softStale ? 'Not seen in recent uplinks (codec may emit conditionally)' : 'No longer reported',
            actionsForStaleMetric(row.application_id, row.device_id, row.opcgw.chirpstack_metric_name, reason),
            softStale
          );
          break;
        case 'available':
          appendAvailableRow(
            scope,
            row.chirpstack_observed.key,
            'inferred ' + row.chirpstack_observed.inferred_wire_type,
            actionsForAvailableMetric(row.application_id, row.device_id, row.chirpstack_observed.key)
          );
          break;
        case 'drifted': {
          const opcgwVal = row.opcgw.metric_type;
          const csVal = row.chirpstack_observed
            ? row.chirpstack_observed.inferred_wire_type
            : row.drift_details && row.drift_details.inferred_type
            ? row.drift_details.inferred_type
            : '(unknown)';
          appendDriftedRow(
            scope,
            row.opcgw.chirpstack_metric_name,
            opcgwVal,
            csVal,
            actionsForDriftedMetric(row.application_id, row.device_id, row.opcgw.chirpstack_metric_name, csVal)
          );
          break;
        }
      }
    }
  }

  /* ----- Confirmation modal pump. */

  function confirmDestructive(title, message, onYes) {
    const dialog = document.getElementById('drift-confirm-modal');
    if (!dialog) {
      onYes();
      return;
    }
    document.getElementById('drift-confirm-title').textContent = title;
    document.getElementById('drift-confirm-message').textContent = message;
    const yesBtn = document.getElementById('drift-confirm-yes');
    const noBtn = document.getElementById('drift-confirm-cancel');
    const cleanup = () => {
      yesBtn.removeEventListener('click', onYesClick);
      noBtn.removeEventListener('click', onNoClick);
      dialog.close();
    };
    const onYesClick = () => {
      cleanup();
      onYes();
    };
    const onNoClick = () => cleanup();
    yesBtn.addEventListener('click', onYesClick);
    noBtn.addEventListener('click', onNoClick);
    dialog.showModal();
  }

  /* ----- Action handlers (event delegation on table click). */

  async function handleAction(btn) {
    const act = btn.getAttribute('data-act');
    if (!act) return;
    const appId = btn.getAttribute('data-app-id') || '';
    const devId = btn.getAttribute('data-dev-id') || '';
    const devEui = btn.getAttribute('data-dev-eui') || '';
    const metricKey = btn.getAttribute('data-metric-key') || '';

    if (act === 'add-app') {
      const csName = btn.getAttribute('data-app-name') || '';
      const url =
        '/applications.html?prefill_app_id=' +
        encodeURIComponent(appId) +
        '&prefill_name=' +
        encodeURIComponent(csName);
      await emitAudit('drift_action', {
        action: 'deep_link_add',
        resource_type: 'application',
        application_id: appId,
      });
      window.location.href = url;
      return;
    }
    if (act === 'add-dev') {
      const devName = btn.getAttribute('data-dev-name') || '';
      const url =
        '/devices-config.html?prefill_app_id=' +
        encodeURIComponent(appId) +
        '&prefill_dev_eui=' +
        encodeURIComponent(devEui) +
        '&prefill_name=' +
        encodeURIComponent(devName);
      await emitAudit('drift_action', {
        action: 'deep_link_add',
        resource_type: 'device',
        application_id: appId,
        device_id: devEui,
      });
      window.location.href = url;
      return;
    }
    if (act === 'add-metric') {
      const url =
        '/devices-config.html?prefill_app_id=' +
        encodeURIComponent(appId) +
        '&prefill_dev_eui=' +
        encodeURIComponent(devId) +
        '&prefill_metric_key=' +
        encodeURIComponent(metricKey);
      await emitAudit('drift_action', {
        action: 'deep_link_add',
        resource_type: 'metric',
        application_id: appId,
        device_id: devId,
        metric_name: metricKey,
      });
      window.location.href = url;
      return;
    }
    if (act === 'remove-app') {
      confirmDestructive(
        'Remove application from opcgw?',
        "Remove application '" +
          appId +
          "' from opcgw? This removes the application AND all its configured devices and metrics. Resources are not deleted from ChirpStack — they will reappear here as 'available' on the next refresh.",
        async () => {
          await emitAudit('drift_action', {
            action: 'remove',
            resource_type: 'application',
            application_id: appId,
          });
          const resp = await fetch('/api/applications/' + encodeURIComponent(appId), {
            method: 'DELETE',
            headers: { 'Content-Type': 'application/json' },
          });
          if (resp.ok) {
            loadDrift();
          } else {
            alert('Remove failed: HTTP ' + resp.status);
          }
        }
      );
      return;
    }
    if (act === 'remove-dev') {
      confirmDestructive(
        'Remove device from opcgw?',
        "Remove device '" +
          devId +
          "' (under application '" +
          appId +
          "') from opcgw? This removes the device AND its configured metrics.",
        async () => {
          await emitAudit('drift_action', {
            action: 'remove',
            resource_type: 'device',
            application_id: appId,
            device_id: devId,
          });
          const resp = await fetch(
            '/api/applications/' +
              encodeURIComponent(appId) +
              '/devices/' +
              encodeURIComponent(devId),
            { method: 'DELETE', headers: { 'Content-Type': 'application/json' } }
          );
          if (resp.ok) {
            loadDrift();
          } else {
            alert('Remove failed: HTTP ' + resp.status);
          }
        }
      );
      return;
    }
    if (act === 'rename-app') {
      const newName = btn.getAttribute('data-new-name') || '';
      await emitAudit('drift_action', {
        action: 'update_name',
        resource_type: 'application',
        application_id: appId,
        operator_choice: newName,
      });
      const resp = await fetch('/api/applications/' + encodeURIComponent(appId), {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ application_name: newName }),
      });
      if (resp.ok) {
        loadDrift();
      } else {
        alert('Rename failed: HTTP ' + resp.status);
      }
      return;
    }
    if (act === 'keep-stale-app') {
      await emitAudit('drift_dismissed', {
        class: 'stale',
        resource_type: 'application',
        application_id: appId,
      });
      btn.disabled = true;
      btn.textContent = 'Dismissed';
      return;
    }
    if (act === 'keep-drifted-app') {
      await emitAudit('drift_dismissed', {
        class: 'drifted',
        resource_type: 'application',
        application_id: appId,
        drift_reason: 'name_differs',
      });
      btn.disabled = true;
      btn.textContent = 'Dismissed';
      return;
    }
    if (act === 'keep-stale-dev' || act === 'keep-drifted-dev') {
      const reason =
        act === 'keep-stale-dev'
          ? btn.getAttribute('data-reason') || 'stale'
          : 'name_differs';
      await emitAudit('drift_dismissed', {
        class: act === 'keep-stale-dev' ? 'stale' : 'drifted',
        resource_type: 'device',
        application_id: appId,
        device_id: devId,
        drift_reason: reason,
      });
      btn.disabled = true;
      btn.textContent = 'Dismissed';
      return;
    }
    if (act === 'keep-stale-metric' || act === 'keep-drifted-metric') {
      const reason =
        btn.getAttribute('data-reason') ||
        (act === 'keep-stale-metric' ? 'not_in_recent_uplinks' : 'wire_type_mismatch');
      await emitAudit('drift_dismissed', {
        class: act === 'keep-stale-metric' ? 'stale' : 'drifted',
        resource_type: 'metric',
        application_id: appId,
        device_id: devId,
        metric_name: metricKey,
        drift_reason: reason,
      });
      btn.disabled = true;
      btn.textContent = 'Dismissed';
      return;
    }
    if (act === 'remove-metric' || act === 'update-metric-type') {
      // These actions need to PUT the entire device (Story 9-5 contract:
      // device PUT replaces the read_metric_list). v1 of the drift view
      // surfaces them as "edit on the devices-config page" deep-links
      // rather than carrying out the full PUT cycle inline — keeps the
      // C-4 surface read-only-with-audit and reuses the canonical edit
      // flow.
      const url =
        '/devices-config.html?prefill_app_id=' +
        encodeURIComponent(appId) +
        '&prefill_dev_eui=' +
        encodeURIComponent(devId);
      await emitAudit('drift_action', {
        action: act === 'remove-metric' ? 'remove' : 'update_wire_type',
        resource_type: 'metric',
        application_id: appId,
        device_id: devId,
        metric_name: metricKey,
      });
      window.location.href = url;
      return;
    }
  }

  /* ----- Fetch + render orchestration. */

  async function loadDrift() {
    setText('drift-fetched-at', 'Loading…');
    hide('drift-error');
    hide('drift-unreachable');
    hide('drift-large-banner');
    try {
      const resp = await fetch('/api/inventory/drift', {
        method: 'GET',
        credentials: 'include',
      });
      if (!resp.ok) {
        show('drift-error');
        document.getElementById('drift-error').textContent =
          'Drift fetch failed: HTTP ' + resp.status;
        setText('drift-fetched-at', '(failed)');
        return;
      }
      const body = await resp.json();
      state.lastResponse = body;
      state.chirpstackReachable = !!body.chirpstack_reachable;
      setText('drift-fetched-at', 'Last refreshed at ' + body.fetched_at);
      renderSummary(body.summary || { ok: 0, stale: 0, available: 0, drifted: 0, total: 0 });
      if (!state.chirpstackReachable) show('drift-unreachable');
      if (body.summary && body.summary.total > 500) {
        show('drift-large-banner');
        setText('drift-large-total', body.summary.total);
      }
      renderRows(body);
    } catch (e) {
      show('drift-error');
      document.getElementById('drift-error').textContent =
        'Drift fetch error: ' + (e && e.message ? e.message : 'unknown');
    }
  }

  function wireEvents() {
    const refreshBtn = document.getElementById('drift-refresh');
    if (refreshBtn) refreshBtn.addEventListener('click', loadDrift);
    const retryBtn = document.getElementById('drift-retry');
    if (retryBtn) retryBtn.addEventListener('click', loadDrift);
    document.addEventListener('click', (ev) => {
      const target = ev.target;
      if (target && target.tagName === 'BUTTON' && target.hasAttribute('data-act')) {
        handleAction(target);
      }
    });
  }

  document.addEventListener('DOMContentLoaded', () => {
    wireEvents();
    loadDrift();
  });
})();
