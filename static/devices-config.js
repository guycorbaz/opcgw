// SPDX-License-Identifier: MIT OR Apache-2.0
// (c) [2024] Guy Corbaz
//
// Story 9-5 — Device + metric mapping CRUD page controller.
// Vanilla JS, no SPA framework, no build step.

(function () {
  'use strict';

  const METRIC_TYPES = ['Float', 'Int', 'Bool', 'String'];

  function el(tag, attrs, children) {
    const node = document.createElement(tag);
    if (attrs) {
      for (const k of Object.keys(attrs)) {
        if (k === 'class') node.className = attrs[k];
        else if (k === 'text') node.textContent = attrs[k];
        else if (k === 'html') node.innerHTML = attrs[k];
        else if (k.startsWith('on')) node.addEventListener(k.slice(2), attrs[k]);
        else node.setAttribute(k, attrs[k]);
      }
    }
    if (children) for (const c of children) if (c) node.appendChild(c);
    return node;
  }

  function showError(banner, message) {
    banner.textContent = message;
    banner.hidden = false;
  }
  function clearError(banner) {
    banner.textContent = '';
    banner.hidden = true;
  }

  async function fetchJson(url, opts) {
    const res = await fetch(url, opts || {});
    let body = null;
    let parseError = null;
    // Iter-1 review L6 (Blind B17 + Edge E4-E6): distinguish a real
    // parse failure (HTML 500 page, proxy interstitial) from a
    // legitimate no-body response. Iter-2 review L2: also treat a
    // zero-byte body on any status as no-body (some endpoints /
    // proxies return 200 + Content-Length: 0 for "no content"
    // semantics rather than the strict 204).
    const contentLength = res.headers.get('content-length');
    const isEmptyBody = res.status === 204 || contentLength === '0';
    if (!isEmptyBody) {
      try { body = await res.json(); } catch (e) { parseError = e; }
    }
    return { status: res.status, body, parseError, headers: res.headers };
  }

  async function loadApplications() {
    const result = await fetchJson('/api/applications');
    if (result.status !== 200) {
      throw new Error('GET /api/applications failed (status ' + result.status + ')');
    }
    if (!result.body) {
      throw new Error('GET /api/applications: empty or non-JSON body'
        + (result.parseError ? ' (' + result.parseError.message + ')' : ''));
    }
    return result.body.applications || [];
  }

  async function loadDevices(applicationId) {
    const url = '/api/applications/' + encodeURIComponent(applicationId) + '/devices';
    const result = await fetchJson(url);
    if (result.status !== 200) {
      throw new Error('GET ' + url + ' failed (status ' + result.status + ')');
    }
    if (!result.body) {
      throw new Error('GET ' + url + ': empty or non-JSON body'
        + (result.parseError ? ' (' + result.parseError.message + ')' : ''));
    }
    return result.body.devices || [];
  }

  async function loadDevice(applicationId, deviceId) {
    const url = '/api/applications/' + encodeURIComponent(applicationId)
      + '/devices/' + encodeURIComponent(deviceId);
    const result = await fetchJson(url);
    if (result.status !== 200) {
      throw new Error('GET ' + url + ' failed (status ' + result.status + ')');
    }
    if (!result.body) {
      throw new Error('GET ' + url + ': empty or non-JSON body'
        + (result.parseError ? ' (' + result.parseError.message + ')' : ''));
    }
    return result.body;
  }

  function buildMetricRow(metric, container) {
    const row = el('div', { class: 'metric-row' });

    const nameWrap = el('div', null, [
      el('label', { text: 'metric_name' }),
      el('input', {
        type: 'text', name: 'metric_name',
        value: (metric && metric.metric_name) || '',
        required: 'required',
      }),
    ]);
    const cpsNameWrap = el('div', null, [
      el('label', { text: 'chirpstack_metric_name' }),
      el('input', {
        type: 'text', name: 'chirpstack_metric_name',
        value: (metric && metric.chirpstack_metric_name) || '',
        required: 'required',
      }),
    ]);
    const typeSelect = el('select', { name: 'metric_type' });
    for (const t of METRIC_TYPES) {
      const opt = el('option', { value: t, text: t });
      if (metric && metric.metric_type === t) opt.selected = true;
      typeSelect.appendChild(opt);
    }
    const typeWrap = el('div', null, [
      el('label', { text: 'metric_type' }),
      typeSelect,
    ]);
    const unitWrap = el('div', null, [
      el('label', { text: 'metric_unit (optional)' }),
      el('input', {
        type: 'text', name: 'metric_unit',
        value: (metric && metric.metric_unit) || '',
      }),
    ]);
    const removeBtn = el('button', {
      type: 'button',
      class: 'btn-remove-metric',
      text: '×',
      title: 'Remove this metric',
      onclick: function () { row.remove(); },
    });

    row.appendChild(nameWrap);
    row.appendChild(cpsNameWrap);
    row.appendChild(typeWrap);
    row.appendChild(unitWrap);
    row.appendChild(removeBtn);
    container.appendChild(row);
  }

  function readMetricsFromContainer(container) {
    const rows = container.querySelectorAll('.metric-row');
    const result = [];
    for (const row of rows) {
      const obj = {};
      obj.metric_name = row.querySelector('input[name="metric_name"]').value.trim();
      obj.chirpstack_metric_name = row.querySelector('input[name="chirpstack_metric_name"]').value.trim();
      obj.metric_type = row.querySelector('select[name="metric_type"]').value;
      // Iter-1 review L7 (Edge E8): trim metric_unit so a
      // whitespace-only field doesn't persist as a visually-empty
      // cell on the next read.
      const unit = row.querySelector('input[name="metric_unit"]').value.trim();
      if (unit.length > 0) obj.metric_unit = unit;
      result.push(obj);
    }
    return result;
  }

  async function deleteDevice(applicationId, deviceId) {
    if (!window.confirm('Delete device "' + deviceId + '"? Orphaned metric values may persist until the next pruning cycle.')) {
      return;
    }
    const url = '/api/applications/' + encodeURIComponent(applicationId)
      + '/devices/' + encodeURIComponent(deviceId);
    const res = await fetch(url, {
      method: 'DELETE',
      headers: { 'Content-Type': 'application/json' },
    });
    if (res.status !== 204) {
      const body = await res.json().catch(function () { return null; });
      const msg = body && body.error ? body.error : ('DELETE failed: ' + res.status);
      window.alert(msg);
      return;
    }
    await render();
  }

  function buildApplicationSection(app) {
    const section = el('section', {
      class: 'application-section',
      'data-application-id': app.application_id,
    });
    section.appendChild(el('h2', {
      text: app.application_name + ' (' + app.application_id + ')',
    }));

    const tbody = el('tbody');
    const table = el('table', { class: 'devices' }, [
      el('thead', null, [
        el('tr', null, [
          el('th', { text: 'Device ID' }),
          el('th', { text: 'Device name' }),
          el('th', { text: 'Metrics' }),
          el('th', { text: 'Actions' }),
        ]),
      ]),
      tbody,
    ]);
    section.appendChild(table);

    const placeholder = el('tr', null, [
      el('td', { colspan: '4', text: 'Loading devices…' }),
    ]);
    tbody.appendChild(placeholder);

    loadDevices(app.application_id).then(function (devices) {
      tbody.replaceChildren();
      if (devices.length === 0) {
        tbody.appendChild(el('tr', null, [
          el('td', { colspan: '4', text: '(no devices configured)' }),
        ]));
      }
      for (const d of devices) {
        const editBtn = el('button', {
          type: 'button', class: 'btn-edit', text: 'Edit',
          onclick: function () { openEditModal(app.application_id, d.device_id); },
        });
        const deleteBtn = el('button', {
          type: 'button', class: 'btn-delete', text: 'Delete',
          onclick: function () { deleteDevice(app.application_id, d.device_id); },
        });
        const tr = el('tr', null, [
          el('td', { text: d.device_id }),
          el('td', { text: d.device_name }),
          el('td', { text: String(d.metric_count) }),
          el('td', { class: 'actions' }, [editBtn, deleteBtn]),
        ]);
        tbody.appendChild(tr);
      }
    }).catch(function (err) {
      tbody.replaceChildren();
      tbody.appendChild(el('tr', null, [
        el('td', {
          colspan: '4',
          class: 'error-banner',
          text: 'Failed to load devices: ' + err.message,
        }),
      ]));
    });

    // Per-application create form.
    const createErr = el('div', { class: 'error-banner', hidden: 'hidden' });
    const metricContainer = el('div');
    const form = el('form', {
      class: 'crud-form',
      action: '/api/applications/' + encodeURIComponent(app.application_id) + '/devices',
      method: 'POST',
    });
    form.appendChild(el('h3', { text: 'Add device' }));
    form.appendChild(el('label', { text: 'Device ID (DevEUI)' }));
    form.appendChild(el('input', {
      type: 'text', name: 'device_id', required: 'required',
    }));
    form.appendChild(el('label', { text: 'Device name' }));
    form.appendChild(el('input', {
      type: 'text', name: 'device_name', required: 'required',
    }));
    form.appendChild(el('h4', { text: 'Metric mappings (optional)' }));
    form.appendChild(metricContainer);
    form.appendChild(el('button', {
      type: 'button', class: 'btn-add-metric', text: '+ Add metric',
      onclick: function () { buildMetricRow(null, metricContainer); },
    }));
    form.appendChild(el('button', { type: 'submit', text: 'Create device' }));
    form.appendChild(createErr);
    form.addEventListener('submit', function (ev) {
      ev.preventDefault();
      clearError(createErr);
      const fd = new FormData(form);
      const payload = {
        device_id: String(fd.get('device_id') || '').trim(),
        device_name: String(fd.get('device_name') || '').trim(),
        read_metric_list: readMetricsFromContainer(metricContainer),
      };
      const url = form.getAttribute('action');
      fetch(url, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload),
      }).then(async function (res) {
        if (res.status !== 201) {
          const body = await res.json().catch(function () { return null; });
          const msg = body && body.error ? body.error : ('POST failed: ' + res.status);
          showError(createErr, msg);
          return;
        }
        form.reset();
        metricContainer.replaceChildren();
        await render();
      }).catch(function (err) {
        showError(createErr, 'Network error: ' + err.message);
      });
    });
    section.appendChild(form);
    return section;
  }

  // Iter-1 review L2 (Blind B7): double-click guard so back-to-back
  // Edit clicks don't fire concurrent loadDevice fetches that race
  // each other's metric-row population. The flag is reset in
  // closeEditModal AND on the catch path below so a network error
  // doesn't leave the modal permanently inert.
  let editModalLoading = false;

  async function openEditModal(applicationId, deviceId) {
    if (editModalLoading) return;
    editModalLoading = true;
    // Iter-2 review M4: wrap the entire body in try/finally so a
    // synchronous DOM-null deref above the inner try block (e.g.,
    // ad-blocker stripped the dialog markup, partial render failure)
    // does NOT leave editModalLoading=true — that would silently
    // deadlock every subsequent Edit click and force a page reload.
    try {
      const modal = document.getElementById('edit-modal');
      const errBanner = document.getElementById('edit-error');
      clearError(errBanner);
      document.getElementById('edit-application-id').value = applicationId;
      document.getElementById('edit-device-id').value = deviceId;
      document.getElementById('edit-modal-title').textContent =
        'Edit device "' + deviceId + '"';
      const metricContainer = document.getElementById('edit-metrics-container');
      metricContainer.replaceChildren();

      try {
        const dev = await loadDevice(applicationId, deviceId);
        document.getElementById('edit-device-name').value = dev.device_name || '';
        for (const m of dev.read_metric_list || []) buildMetricRow(m, metricContainer);
      } catch (err) {
        showError(errBanner, err.message);
      }
      // Iter-1 review L1 (Blind B6): use the HTMLDialogElement API
      // (showModal) instead of `setAttribute('open')` so focus-trap,
      // ESC-to-close, and aria-modal semantics work for keyboard /
      // screen-reader users. Fallback to attribute toggle if the
      // browser doesn't support the dialog API.
      if (typeof modal.showModal === 'function') {
        try { modal.showModal(); } catch (_) { modal.setAttribute('open', 'open'); }
      } else {
        modal.setAttribute('open', 'open');
      }
    } finally {
      editModalLoading = false;
    }
  }

  function closeEditModal() {
    const modal = document.getElementById('edit-modal');
    if (typeof modal.close === 'function') {
      try { modal.close(); } catch (_) { modal.removeAttribute('open'); }
    } else {
      modal.removeAttribute('open');
    }
    editModalLoading = false;
  }

  async function submitEdit(ev) {
    ev.preventDefault();
    const errBanner = document.getElementById('edit-error');
    clearError(errBanner);
    const applicationId = document.getElementById('edit-application-id').value;
    const deviceId = document.getElementById('edit-device-id').value;
    const deviceName = document.getElementById('edit-device-name').value.trim();
    const metrics = readMetricsFromContainer(
      document.getElementById('edit-metrics-container'),
    );
    const url = '/api/applications/' + encodeURIComponent(applicationId)
      + '/devices/' + encodeURIComponent(deviceId);
    try {
      const res = await fetch(url, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          device_name: deviceName,
          read_metric_list: metrics,
        }),
      });
      if (res.status !== 200) {
        const body = await res.json().catch(function () { return null; });
        showError(errBanner, (body && body.error) || ('PUT failed: ' + res.status));
        return;
      }
      closeEditModal();
      await render();
    } catch (err) {
      showError(errBanner, 'Network error: ' + err.message);
    }
  }

  async function render() {
    const container = document.getElementById('applications-container');
    const banner = document.getElementById('list-error');
    clearError(banner);
    container.replaceChildren();
    container.appendChild(el('p', { class: 'loading', text: 'Loading…' }));
    try {
      const apps = await loadApplications();
      container.replaceChildren();
      if (apps.length === 0) {
        container.appendChild(el('p', {
          text: 'No applications configured. Create one via /applications.html first.',
        }));
        return;
      }
      for (const app of apps) container.appendChild(buildApplicationSection(app));
    } catch (err) {
      container.replaceChildren();
      showError(banner, 'Failed to load applications: ' + err.message);
    }
  }

  document.addEventListener('DOMContentLoaded', function () {
    document.getElementById('edit-form').addEventListener('submit', submitEdit);
    document.getElementById('edit-cancel').addEventListener('click', closeEditModal);
    document.getElementById('edit-add-metric').addEventListener('click', function () {
      buildMetricRow(null, document.getElementById('edit-metrics-container'));
    });
    render();
  });
})();
