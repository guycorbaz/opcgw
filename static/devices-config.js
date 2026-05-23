// SPDX-License-Identifier: MIT OR Apache-2.0
// (c) [2024] Guy Corbaz
//
// Story 9-5 — Device + metric mapping CRUD page controller.
// Story C-2 — Device + metric pickers fed by /api/inventory/*.
// Vanilla JS, no SPA framework, no build step.

(function () {
  'use strict';

  const METRIC_TYPES = ['Float', 'Int', 'Bool', 'String'];
  // Story C-2: per-page localStorage keys for picker-vs-manual mode.
  const DEVICES_PAGE_KEY = 'devices';
  const METRICS_PAGE_KEY = 'metrics';
  // Story C-2: namespace-aliased global from inventory-picker.js (must
  // be loaded BEFORE this script — see static/devices-config.html).
  const picker = window.opcgwPicker;

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
    // Story C-2: per-form picker state object captures all the
    // operator-set bits the submit handler needs to read at submit
    // time (NOT at picker-render time, AC#14).
    //
    // Iter-1 review HIGH-7: `deviceFetchController` / `uplinkFetchController`
    // hold the AbortController for the most recent inventory fetch
    // per-picker so a rapid second selection / refresh aborts the
    // first request (prevents stale-wins races where slower-resolving
    // fetch overwrites the freshly-clicked one).
    const pickerState = {
      mode: picker.mode.get(DEVICES_PAGE_KEY),
      metricsMode: picker.mode.get(METRICS_PAGE_KEY),
      pickedDevices: [],   // [{id, name, dev_eui}]
      observedKeys: [],    // [{key, wire_type, sample_value}]
      currentDevEui: '',   // selected dev_eui (lowercase, normalized by API)
      deviceFetchController: null,  // iter-1 HIGH-7
      uplinkFetchController: null,  // iter-1 HIGH-7
    };
    const form = el('form', {
      class: 'crud-form',
      action: '/api/applications/' + encodeURIComponent(app.application_id) + '/devices',
      method: 'POST',
    });
    form.appendChild(el('h3', { text: 'Add device' }));

    // -------- Device picker (Story C-2 AC#4 / #5 / #6) --------
    const devPickerWrap = el('div');
    const devPickerToolbar = el('div', { class: 'picker-toolbar' });
    const devPickerSelect = el('select', { 'aria-label': 'Device from ChirpStack' });
    devPickerSelect.disabled = true;
    // Iter-1 review HIGH-7: explicit value="" on the Loading
    // placeholder so a form submit during initial loading state
    // never POSTs the literal string "Loading…" as device_id.
    devPickerSelect.appendChild(el('option', { value: '', text: 'Loading…' }));
    const devPickerRefresh = el('button', {
      type: 'button', text: '↻', title: 'Refresh from ChirpStack (cache bypass)',
    });
    const devPickerToManual = el('a', {
      role: 'button', tabindex: '0', text: 'Switch to manual entry',
    });
    devPickerToolbar.appendChild(devPickerSelect);
    devPickerToolbar.appendChild(devPickerRefresh);
    devPickerToolbar.appendChild(devPickerToManual);
    devPickerWrap.appendChild(devPickerToolbar);
    const devEuiFootnote = el('div', { class: 'dev-eui-footnote', text: '' });
    devPickerWrap.appendChild(devEuiFootnote);
    const devPickerBanner = el('div', { class: 'picker-fallback-banner' });
    devPickerBanner.hidden = true;
    devPickerWrap.appendChild(devPickerBanner);

    const devManualWrap = el('div');
    devManualWrap.hidden = true;
    const devManualToolbar = el('div', { class: 'picker-toolbar' });
    const devManualToPicker = el('a', {
      role: 'button', tabindex: '0', text: 'Switch to picker',
    });
    devManualToolbar.appendChild(devManualToPicker);
    devManualWrap.appendChild(devManualToolbar);
    const devIdInput = el('input', {
      type: 'text', name: 'device_id', placeholder: 'Device ID (DevEUI hex)',
    });
    devManualWrap.appendChild(el('label', { text: 'Device ID (DevEUI)' }));
    devManualWrap.appendChild(devIdInput);

    form.appendChild(el('label', { text: 'Device from ChirpStack' }));
    form.appendChild(devPickerWrap);
    form.appendChild(devManualWrap);

    form.appendChild(el('label', { text: 'Device name' }));
    const devNameInput = el('input', {
      type: 'text', name: 'device_name', required: 'required',
    });
    form.appendChild(devNameInput);
    picker.editedFlag.attach(devNameInput);

    // -------- Metric picker (Story C-2 AC#7 / #8 / #9 / #10) --------
    const metricPickerWrap = el('div');
    const metricPickerToolbar = el('div', { class: 'picker-toolbar' });
    const metricPickerRefresh = el('button', {
      type: 'button', text: '↻ Refresh metric picker',
      title: 'Re-fetch recent uplinks for the selected device',
    });
    const metricPickerToManual = el('a', {
      role: 'button', tabindex: '0', text: 'Switch to manual metric entry',
    });
    metricPickerToolbar.appendChild(metricPickerRefresh);
    metricPickerToolbar.appendChild(metricPickerToManual);
    metricPickerWrap.appendChild(metricPickerToolbar);
    const metricPickerStatus = el('div', { text: 'Choose a device above first.' });
    metricPickerWrap.appendChild(metricPickerStatus);
    const metricPickerRows = el('div');
    metricPickerWrap.appendChild(metricPickerRows);
    const metricPickerBanner = el('div', { class: 'picker-fallback-banner' });
    metricPickerBanner.hidden = true;
    metricPickerWrap.appendChild(metricPickerBanner);

    const metricManualWrap = el('div');
    const metricManualToolbar = el('div', { class: 'picker-toolbar' });
    const metricManualToPicker = el('a', {
      role: 'button', tabindex: '0', text: 'Switch to metric picker',
    });
    metricManualToolbar.appendChild(metricManualToPicker);
    metricManualWrap.appendChild(metricManualToolbar);
    metricManualWrap.appendChild(el('h4', { text: 'Metric mappings (manual)' }));
    metricManualWrap.appendChild(metricContainer);
    metricManualWrap.appendChild(el('button', {
      type: 'button', class: 'btn-add-metric', text: '+ Add metric',
      onclick: function () { buildMetricRow(null, metricContainer); },
    }));

    form.appendChild(el('h4', { text: 'Metrics from recent uplinks (picker)' }));
    form.appendChild(metricPickerWrap);
    form.appendChild(metricManualWrap);

    function applyDeviceMode(mode) {
      pickerState.mode = mode;
      picker.mode.set(DEVICES_PAGE_KEY, mode);
      if (mode === 'manual') {
        devPickerWrap.hidden = true;
        devManualWrap.hidden = false;
      } else {
        devPickerWrap.hidden = false;
        devManualWrap.hidden = true;
      }
    }
    function applyMetricsMode(mode) {
      pickerState.metricsMode = mode;
      picker.mode.set(METRICS_PAGE_KEY, mode);
      if (mode === 'manual') {
        metricPickerWrap.hidden = true;
        metricManualWrap.hidden = false;
      } else {
        metricPickerWrap.hidden = false;
        metricManualWrap.hidden = true;
      }
    }

    function setDevicePickerBanner(msg, withRetry) {
      devPickerBanner.replaceChildren();
      if (!msg) {
        devPickerBanner.hidden = true;
        return;
      }
      devPickerBanner.appendChild(document.createTextNode(msg + ' '));
      if (withRetry) {
        const btn = el('button', {
          type: 'button', text: 'Retry picker',
          onclick: function () { loadDevicePicker({ refresh: true }); },
        });
        devPickerBanner.appendChild(btn);
      }
      devPickerBanner.hidden = false;
    }

    function setMetricPickerStatus(msg) {
      metricPickerStatus.textContent = msg || '';
    }

    function setMetricPickerBanner(msg) {
      metricPickerBanner.textContent = msg || '';
      metricPickerBanner.hidden = !msg;
    }

    async function loadDevicePicker(opts) {
      // Iter-1 review HIGH-7: abort prior in-flight fetch.
      if (pickerState.deviceFetchController) {
        pickerState.deviceFetchController.abort();
      }
      const controller = new AbortController();
      pickerState.deviceFetchController = controller;
      setFormSubmitEnabled(false);
      devPickerSelect.disabled = true;
      devPickerSelect.replaceChildren();
      // Iter-1 review HIGH-7: Loading option carries value="" so
      // mid-load submit is safe.
      devPickerSelect.appendChild(el('option', { value: '', text: 'Loading…' }));
      setDevicePickerBanner('');
      try {
        const data = await picker.fetchDevices(app.application_id, {
          refresh: !!(opts && opts.refresh),
        });
        if (controller.signal.aborted) return; // iter-1 HIGH-7
        picker.auditEvent('picker_opened', {
          picker_resource: 'device',
          application_id: app.application_id,
          cache_status: (opts && opts.refresh) ? 'bypassed' : (data.cache_status || 'unknown'),
        });
        if (!data.items || data.items.length === 0) {
          devPickerSelect.replaceChildren();
          devPickerSelect.appendChild(el('option', { text: '(no devices in ChirpStack)' }));
          applyDeviceMode('manual');
          setDevicePickerBanner(
            'No devices found under this application in ChirpStack.',
            true,
          );
          picker.auditEvent('picker_manual_fallback', {
            picker_resource: 'device',
            reason: 'chirpstack_empty',
          });
          return;
        }
        pickerState.pickedDevices = data.items;
        const items = data.items.slice().sort(function (a, b) {
          const ax = (a.name || '').toLowerCase();
          const bx = (b.name || '').toLowerCase();
          return ax < bx ? -1 : ax > bx ? 1 : 0;
        });
        devPickerSelect.replaceChildren();
        const placeholder = el('option', { value: '', text: 'Choose a device…' });
        placeholder.disabled = true;
        placeholder.selected = true;
        devPickerSelect.appendChild(placeholder);
        items.forEach(function (item) {
          const opt = el('option', { value: item.id, text: item.name });
          opt.dataset.devName = item.name;
          // The C-1 InventoryDevice carries id == dev_eui (normalised
          // lowercase hex). The /uplinks endpoint expects the same.
          opt.dataset.devEui = item.id;
          devPickerSelect.appendChild(opt);
        });
        devPickerSelect.disabled = false;
        setFormSubmitEnabled(true);
      } catch (err) {
        if (controller.signal.aborted) return; // iter-1 HIGH-7
        applyDeviceMode('manual');
        setFormSubmitEnabled(true);
        setDevicePickerBanner('Could not reach ChirpStack — switched to manual entry.', true);
        picker.auditEvent('picker_manual_fallback', {
          picker_resource: 'device',
          reason: err && err.status === 502 ? 'chirpstack_unreachable' : 'chirpstack_error',
          error_detail: err && err.message ? String(err.message).slice(0, 200) : '',
        });
      }
    }

    async function loadMetricPicker(devEui, opts) {
      pickerState.observedKeys = [];
      pickerState.currentDevEui = devEui;
      metricPickerRows.replaceChildren();
      setMetricPickerBanner('');
      if (!devEui) {
        setMetricPickerStatus('Choose a device above first.');
        return;
      }
      // Iter-1 review HIGH-7: abort prior in-flight uplinks fetch.
      if (pickerState.uplinkFetchController) {
        pickerState.uplinkFetchController.abort();
      }
      const controller = new AbortController();
      pickerState.uplinkFetchController = controller;
      setMetricPickerStatus('Loading recent uplinks…');
      try {
        const data = await picker.fetchUplinks(devEui, { limit: 10 });
        if (controller.signal.aborted) return; // iter-1 HIGH-7
        picker.auditEvent('picker_opened', {
          picker_resource: 'uplink',
          application_id: app.application_id,
          dev_eui: devEui,
          cache_status: 'bypassed',
        });
        if (!data.observed_keys || data.observed_keys.length === 0) {
          // AC#9: empty observed keys -> flip to manual entry.
          setMetricPickerStatus(
            'No recent uplinks for this device. Either wait for it to send and refresh, or add metrics manually below.',
          );
          applyMetricsMode('manual');
          picker.auditEvent('picker_manual_fallback', {
            picker_resource: 'uplink',
            reason: 'no_recent_uplinks',
          });
          return;
        }
        pickerState.observedKeys = data.observed_keys;
        setMetricPickerStatus('Tick metric keys to include; override wire type per row if needed.');
        data.observed_keys.forEach(function (k, idx) {
          const row = el('div', { class: 'metric-pick-row' });
          // Iter-1 review HIGH-6: prefix checkbox id with the parent
          // application_id so multiple application sections rendered
          // on the same page do NOT collide on HTML id (which would
          // cause a <label for="mk-0"> click to toggle the wrong-
          // section checkbox). Use a sanitised app id segment so any
          // exotic chars in application_id can't break attribute syntax.
          const idSafeAppId = String(app.application_id).replace(/[^A-Za-z0-9_-]/g, '_');
          const checkboxId = 'mk-' + idSafeAppId + '-' + idx;
          const checkbox = el('input', {
            type: 'checkbox', id: checkboxId,
            'data-key': k.key,
            'data-inferred': k.wire_type,
          });
          const label = el('label', { for: checkboxId, text: k.key });
          const typeSelect = el('select');
          METRIC_TYPES.forEach(function (t) {
            const opt = el('option', { value: t, text: t });
            if (t === k.wire_type) opt.selected = true;
            typeSelect.appendChild(opt);
          });
          typeSelect.dataset.role = 'wire-type';
          // Iter-1 review MED — cap stringified sample to ~200 chars
          // so a large nested JSON value cannot blow up the DOM /
          // freeze the operator's browser. textContent path is XSS-
          // safe; this is a denial-of-readability guard only.
          const rawSample = JSON.stringify(k.sample_value);
          const sampleText = rawSample.length > 200 ? rawSample.slice(0, 200) + '…' : rawSample;
          const sample = el('span', {
            class: 'sample-cell',
            text: 'sample: ' + sampleText,
          });
          row.appendChild(checkbox);
          row.appendChild(label);
          row.appendChild(typeSelect);
          row.appendChild(sample);
          metricPickerRows.appendChild(row);
        });
      } catch (err) {
        if (controller.signal.aborted) return; // iter-1 HIGH-7
        setMetricPickerStatus('Could not fetch recent uplinks.');
        setMetricPickerBanner('You can still add metrics manually below.');
        applyMetricsMode('manual');
        picker.auditEvent('picker_manual_fallback', {
          picker_resource: 'uplink',
          reason: err && err.status === 502 ? 'chirpstack_unreachable' : 'chirpstack_error',
          error_detail: err && err.message ? String(err.message).slice(0, 200) : '',
        });
      }
    }

    function readPickerMetrics() {
      const rows = metricPickerRows.querySelectorAll('.metric-pick-row');
      const result = [];
      for (const row of rows) {
        const checkbox = row.querySelector('input[type="checkbox"]');
        if (!checkbox || !checkbox.checked) continue;
        const key = checkbox.dataset.key;
        const inferred = checkbox.dataset.inferred || '';
        const chosen = row.querySelector('select[data-role="wire-type"]').value;
        // Iter-1 review MED-1: omit `sample_values_count` rather than
        // sending a hardcoded `1`. The C-1 `/api/inventory/uplinks`
        // response carries one ObservedKey per distinct key (with the
        // LAST sample value seen, not a per-key occurrence count), so
        // we genuinely don't know how many uplinks contributed to the
        // inference. The server-side `unwrap_or(0)` then represents
        // "unknown count" in the audit emit — which is the truthful
        // value. A future C-1 extension that tracks per-key counts can
        // re-add this field with the real number.
        result.push({
          metric_name: key,
          chirpstack_metric_name: key,
          metric_type: chosen,
          picker_metadata: {
            inferred_type: inferred,
            operator_chosen_type: chosen,
          },
        });
      }
      return result;
    }

    // -------- Wire up handlers --------
    devPickerSelect.addEventListener('change', function () {
      const opt = devPickerSelect.options[devPickerSelect.selectedIndex];
      const devName = opt ? (opt.dataset.devName || opt.textContent || '') : '';
      const devEui = opt ? (opt.dataset.devEui || opt.value || '') : '';
      if (!picker.editedFlag.has(devNameInput) && devName) {
        devNameInput.value = devName;
      }
      devEuiFootnote.textContent = devEui ? 'DevEUI: ' + devEui : '';
      // Iter-1 review MED-3: clear stale currentDevEui + metric rows
      // when the operator re-selects the disabled placeholder option
      // (devEui === ''). Otherwise the subsequent metric-refresh
      // click would re-fetch a dead DevEUI.
      if (!devEui) {
        pickerState.currentDevEui = '';
        metricPickerRows.replaceChildren();
        setMetricPickerStatus('Choose a device above first.');
        return;
      }
      // Trigger metric-picker fetch when the device changes.
      if (pickerState.metricsMode === 'picker') {
        loadMetricPicker(devEui, {});
      }
    });
    devPickerRefresh.addEventListener('click', function () {
      // Iter-1 review LOW-1: drop pre-fetch picker_opened emit;
      // loadDevicePicker's post-fetch emit carries the correct
      // server-provided cache_status from the ?refresh=true response.
      loadDevicePicker({ refresh: true });
    });
    devPickerToManual.addEventListener('click', function () {
      applyDeviceMode('manual');
      picker.auditEvent('picker_manual_fallback', {
        picker_resource: 'device',
        reason: 'operator_choice',
      });
    });
    devManualToPicker.addEventListener('click', function () {
      applyDeviceMode('picker');
      loadDevicePicker({});
    });
    metricPickerRefresh.addEventListener('click', function () {
      if (!pickerState.currentDevEui) {
        // Try to read from the picker selection (if any).
        const opt = devPickerSelect.options[devPickerSelect.selectedIndex];
        const eui = opt && opt.dataset ? opt.dataset.devEui : '';
        if (eui) {
          loadMetricPicker(eui, {});
        } else {
          // Iter-1 review LOW-6: surface a status message instead of
          // a silent no-op so the operator knows why nothing happened.
          setMetricPickerStatus('Select a device first.');
        }
        return;
      }
      loadMetricPicker(pickerState.currentDevEui, {});
    });
    metricPickerToManual.addEventListener('click', function () {
      applyMetricsMode('manual');
      picker.auditEvent('picker_manual_fallback', {
        picker_resource: 'uplink',
        reason: 'operator_choice',
      });
    });
    metricManualToPicker.addEventListener('click', function () {
      applyMetricsMode('picker');
      if (pickerState.currentDevEui) {
        loadMetricPicker(pickerState.currentDevEui, {});
      }
    });

    const submitBtn = el('button', { type: 'submit', text: 'Create device' });
    form.appendChild(submitBtn);
    form.appendChild(createErr);

    // Iter-1 review HIGH-7 helper: disable submit while picker fetch
    // is in flight. Picker mode only — manual entry is always allowed.
    function setFormSubmitEnabled(enabled) {
      if (pickerState.mode === 'manual') {
        submitBtn.disabled = false; // manual entry is always submittable
        return;
      }
      submitBtn.disabled = !enabled;
    }
    form.addEventListener('submit', function (ev) {
      ev.preventDefault();
      clearError(createErr);
      // Read device id from active mode.
      let deviceId = '';
      let deviceName = String(devNameInput.value || '').trim();
      if (pickerState.mode === 'manual') {
        deviceId = String(devIdInput.value || '').trim();
      } else {
        const opt = devPickerSelect.options[devPickerSelect.selectedIndex];
        deviceId = opt ? (opt.dataset.devEui || opt.value || '') : '';
      }
      if (!deviceId) {
        showError(createErr, 'Choose a device from the picker, or switch to manual entry and type a DevEUI.');
        return;
      }
      if (!deviceName) {
        showError(createErr, 'Device name is required.');
        return;
      }
      let metrics;
      if (pickerState.metricsMode === 'picker') {
        metrics = readPickerMetrics();
      } else {
        metrics = readMetricsFromContainer(metricContainer);
      }
      const payload = {
        device_id: deviceId,
        device_name: deviceName,
        read_metric_list: metrics,
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
        metricPickerRows.replaceChildren();
        picker.editedFlag.reset(devNameInput);
        pickerState.currentDevEui = '';
        await render();
      }).catch(function (err) {
        showError(createErr, 'Network error: ' + err.message);
      });
    });

    section.appendChild(form);

    // Boot the picker modes after the form is in the DOM.
    applyDeviceMode(pickerState.mode);
    applyMetricsMode(pickerState.metricsMode);
    if (pickerState.mode === 'picker') loadDevicePicker({});
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
