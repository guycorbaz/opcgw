// SPDX-License-Identifier: MIT OR Apache-2.0
// (c) [2024] Guy Corbaz
//
// Story G-0: drill-down configuration controller for /config.html.
//
// Consolidates the retired flat pages (applications.html / devices-config.html
// / commands.html) into ONE hierarchical surface: Application -> Device ->
// Metrics + Commands. Vanilla JS, no build step, no framework, no node_modules.
//
// Architecture: a tiny hash router drives three mountable views, each rendered
// fresh into #config-root on navigation:
//   #/                                  -> mountApplications
//   #/app/<application_id>              -> mountDevices (one application)
//   #/app/<application_id>/device/<id>  -> mountDeviceDetail (metrics + commands)
//
// All CRUD uses the existing endpoints unchanged (every write STAGES to SQLite
// via the F-0 staged-apply path; the operator applies via the shared
// apply-bar.js). All mutating fetches send Content-Type: application/json and
// credentials:'include' so the gateway's Origin-based CSRF + Basic auth pass.
// The C-2 picker helpers (window.opcgwPicker) are reused verbatim — modes,
// edited-flag, abort-on-rapid-action, audit, escapeHtml — not reinvented.

(function () {
  'use strict';

  var picker = window.opcgwPicker;
  var METRIC_TYPES = ['Float', 'Int', 'Bool', 'String'];
  var DEVICES_PAGE_KEY = 'devices';
  var METRICS_PAGE_KEY = 'metrics';
  var APPS_PAGE_KEY = 'applications';

  // Page-wide monotonic id sequence for metric-pick checkboxes — collision-free
  // by construction regardless of operator-supplied ids (devices-config iter-2).
  var _metricCheckboxIdSeq = 0;

  // Drift-view deep-link prefill is consumed exactly once per page session
  // (review iter-1: location.search persists across hash changes, so without
  // this the app prefill re-fired on every picker reload / post-create render).
  var appPrefillConsumed = false;

  // -----------------------------------------------------------------------
  // DOM + fetch helpers (ported from the retired controllers).
  // -----------------------------------------------------------------------
  function el(tag, attrs, children) {
    var node = document.createElement(tag);
    if (attrs) {
      Object.keys(attrs).forEach(function (k) {
        if (k === 'class') node.className = attrs[k];
        else if (k === 'text') node.textContent = attrs[k];
        else if (k === 'html') node.innerHTML = attrs[k];
        else if (k.slice(0, 2) === 'on') node.addEventListener(k.slice(2), attrs[k]);
        else node.setAttribute(k, attrs[k]);
      });
    }
    if (children) children.forEach(function (c) { if (c) node.appendChild(c); });
    return node;
  }

  function showError(banner, msg) { banner.textContent = msg; banner.hidden = false; }
  function clearError(banner) { banner.textContent = ''; banner.hidden = true; }

  // Story G-2 (#142): contextual field-help affordance (shared catalog in
  // field-help.js). Returns a node to place next to a field, or null when
  // field-help.js isn't loaded / the key is unknown.
  function fieldHelp(key, inputEl) {
    return (window.opcgwHelp && window.opcgwHelp.affordance(key, inputEl)) || null;
  }
  // Safe append: a null affordance (field-help.js absent / unknown key) is
  // a no-op rather than an appendChild(null) TypeError.
  function appendHelp(parent, key, inputEl) {
    var h = fieldHelp(key, inputEl);
    if (h) parent.appendChild(h);
  }

  var rootError = function () { return document.getElementById('config-error'); };

  // Distinguish a real parse failure (HTML 500 page, proxy interstitial) from a
  // legitimate no-body response (204 / Content-Length: 0). Mirrors
  // devices-config.js::fetchJson.
  async function fetchJson(url, opts) {
    var res = await fetch(url, opts || { credentials: 'include' });
    var body = null;
    var parseError = null;
    var contentLength = res.headers.get('content-length');
    var isEmptyBody = res.status === 204 || contentLength === '0';
    if (!isEmptyBody) {
      try { body = await res.json(); } catch (e) { parseError = e; }
    }
    return { status: res.status, body: body, parseError: parseError, headers: res.headers };
  }

  function jsonHeaders() { return { 'Content-Type': 'application/json' }; }

  // -----------------------------------------------------------------------
  // URL prefill (Story C-4 deep-links from the inventory drift view, carried
  // through the redirect stubs that replaced the old pages).
  // -----------------------------------------------------------------------
  function parsePrefill() {
    var params;
    try { params = new URLSearchParams(window.location.search || ''); }
    catch (e) { return {}; }
    return {
      appId: params.get('prefill_app_id') || '',
      devEui: params.get('prefill_dev_eui') || '',
      devName: params.get('prefill_name') || '',
      metricKey: params.get('prefill_metric_key') || '',
    };
  }

  // -----------------------------------------------------------------------
  // Router.
  // -----------------------------------------------------------------------
  // Tolerate a malformed percent-sequence in a hand-typed/corrupted hash
  // (review iter-1: a bare decodeURIComponent threw URIError out of render()).
  function safeDecode(s) {
    try { return decodeURIComponent(s); } catch (e) { return s; }
  }

  function parseHash() {
    var h = window.location.hash || '';
    if (h.charAt(0) === '#') h = h.slice(1);
    // Forms: "/", "/app/<id>", "/app/<id>/device/<id>"
    var m = h.match(/^\/app\/([^/]+)\/device\/([^/]+)\/?$/);
    if (m) {
      return { level: 'device', appId: safeDecode(m[1]), devId: safeDecode(m[2]) };
    }
    m = h.match(/^\/app\/([^/]+)\/?$/);
    if (m) {
      return { level: 'devices', appId: safeDecode(m[1]) };
    }
    return { level: 'apps' };
  }

  function navTo(hash) { window.location.hash = hash; }

  function setBreadcrumb(parts) {
    var bc = document.getElementById('breadcrumb');
    bc.replaceChildren();
    parts.forEach(function (p, i) {
      if (i > 0) bc.appendChild(el('span', { class: 'sep', text: '/' }));
      if (p.href) {
        bc.appendChild(el('a', { href: p.href, text: p.label }));
      } else {
        bc.appendChild(el('span', { class: 'current', text: p.label }));
      }
    });
  }

  // Per-render AbortController so navigating away cancels in-flight loads
  // (no stale-wins races across drill levels).
  var renderToken = 0;

  async function render() {
    clearError(rootError());
    // Tear down any open command-edit dialog before switching views, so a
    // browser Back/Forward can't leave a modal orphaned over the new view
    // (review iter-1).
    var strayDialog = document.querySelector('dialog.modal[open]');
    if (strayDialog) {
      if (typeof strayDialog.close === 'function') { try { strayDialog.close(); } catch (_) { strayDialog.removeAttribute('open'); } }
      strayDialog.remove();
    }
    var route = parseHash();
    var myToken = ++renderToken;
    var root = document.getElementById('config-root');
    root.replaceChildren(el('p', { class: 'loading', text: 'Loading…' }));
    function stillCurrent() { return myToken === renderToken; }
    try {
      if (route.level === 'apps') {
        await mountApplications(root, stillCurrent);
      } else if (route.level === 'devices') {
        await mountDevices(root, route.appId, stillCurrent);
      } else {
        await mountDeviceDetail(root, route.appId, route.devId, stillCurrent);
      }
    } catch (err) {
      if (!stillCurrent()) return;
      root.replaceChildren();
      showError(rootError(), 'Failed to load: ' + (err && err.message ? err.message : err));
    }
  }

  // =======================================================================
  // VIEW 1 — Applications.
  // =======================================================================
  async function mountApplications(root, stillCurrent) {
    setBreadcrumb([{ label: 'Applications' }]);
    var prefill = parsePrefill();

    var result = await fetchJson('/api/applications');
    if (!stillCurrent()) return;
    if (result.status !== 200 || !result.body) {
      throw new Error('GET /api/applications failed (status ' + result.status + ')');
    }
    var apps = result.body.applications || [];

    root.replaceChildren();

    // ---- Create form (with C-2 application picker) ----
    var createSection = el('section', { class: 'config-section' });
    createSection.appendChild(el('h2', { text: 'Add application' }));
    var createError = el('div', { class: 'error-banner', hidden: 'hidden' });

    var pickerWrap = el('div');
    var pickerToolbar = el('div', { class: 'picker-toolbar' });
    var pickerSelect = el('select', { 'aria-label': 'Application from ChirpStack' });
    pickerSelect.disabled = true;
    pickerSelect.appendChild(el('option', { value: '', text: 'Loading…' }));
    var pickerRefresh = el('button', { type: 'button', text: '↻', title: 'Refresh from ChirpStack (cache bypass)' });
    var toManual = el('a', { role: 'button', tabindex: '0', text: 'Switch to manual entry' });
    pickerToolbar.appendChild(pickerSelect);
    pickerToolbar.appendChild(pickerRefresh);
    pickerToolbar.appendChild(toManual);
    pickerWrap.appendChild(pickerToolbar);
    var pickerBanner = el('div', { class: 'picker-fallback-banner' });
    pickerBanner.hidden = true;
    pickerWrap.appendChild(pickerBanner);

    var manualWrap = el('div');
    manualWrap.hidden = true;
    var manualToolbar = el('div', { class: 'picker-toolbar' });
    var toPicker = el('a', { role: 'button', tabindex: '0', text: 'Switch to picker' });
    manualToolbar.appendChild(toPicker);
    manualWrap.appendChild(manualToolbar);
    var manualInput = el('input', { type: 'text', placeholder: 'Application ID' });
    manualWrap.appendChild(el('label', { text: 'Application ID' }));
    manualWrap.appendChild(manualInput);

    var nameInput = el('input', { type: 'text', required: 'required' });

    var form = el('form', { class: 'crud-form' });
    form.appendChild(el('label', { text: 'Application from ChirpStack' }));
    form.appendChild(pickerWrap);
    form.appendChild(manualWrap);
    form.appendChild(el('label', { text: 'Application name' }));
    form.appendChild(nameInput);
    var submitBtn = el('button', { type: 'submit', text: 'Create application' });
    form.appendChild(submitBtn);
    form.appendChild(createError);
    createSection.appendChild(form);
    picker.editedFlag.attach(nameInput);

    var state = { mode: picker.mode.get(APPS_PAGE_KEY), fetchController: null };

    function applyMode(mode) {
      state.mode = mode;
      picker.mode.set(APPS_PAGE_KEY, mode);
      if (mode === 'manual') { pickerWrap.hidden = true; manualWrap.hidden = false; }
      else { pickerWrap.hidden = false; manualWrap.hidden = true; }
    }
    function setSubmitEnabled(on) { submitBtn.disabled = !on; }
    function setBanner(msg) {
      pickerBanner.replaceChildren();
      if (!msg) { pickerBanner.hidden = true; return; }
      pickerBanner.appendChild(document.createTextNode(msg + ' '));
      pickerBanner.appendChild(el('button', {
        type: 'button', text: 'Retry picker',
        onclick: function () { loadPicker({ refresh: true }).catch(picker.warnUnlessAbort('app picker retry')); },
      }));
      pickerBanner.hidden = false;
    }

    async function loadPicker(opts) {
      if (state.fetchController) state.fetchController.abort();
      var controller = new AbortController();
      state.fetchController = controller;
      setSubmitEnabled(false);
      pickerSelect.disabled = true;
      pickerSelect.replaceChildren(el('option', { value: '', text: 'Loading…' }));
      setBanner(null);
      try {
        try {
          var data = await picker.fetchApplications({ refresh: !!(opts && opts.refresh) });
          if (controller.signal.aborted) return;
          picker.auditEvent('picker_opened', {
            picker_resource: 'application',
            cache_status: data.cache_status || 'unknown',
          });
          if (!data.items || data.items.length === 0) {
            pickerSelect.replaceChildren(el('option', {
              value: '', disabled: 'disabled', selected: 'selected', text: '(no applications in ChirpStack)',
            }));
            applyMode('manual');
            setBanner('No applications found in ChirpStack for this tenant — type one manually or create one in ChirpStack first.');
            picker.auditEvent('picker_manual_fallback', { picker_resource: 'application', reason: 'chirpstack_empty' });
            return;
          }
          var items = data.items.slice().sort(byNameCI);
          pickerSelect.replaceChildren();
          var ph = el('option', { value: '', text: 'Choose an application…' });
          ph.disabled = true; ph.selected = true;
          pickerSelect.appendChild(ph);
          items.forEach(function (item) {
            var opt = el('option', { value: item.id, text: item.name });
            opt.dataset.appName = item.name;
            pickerSelect.appendChild(opt);
          });
          pickerSelect.disabled = false;
          applyAppPrefill();
        } catch (err) {
          if (controller.signal.aborted) return;
          applyMode('manual');
          setBanner('Could not reach ChirpStack — switched to manual entry.');
          picker.auditEvent('picker_manual_fallback', {
            picker_resource: 'application',
            reason: err && err.status === 502 ? 'chirpstack_unreachable' : 'chirpstack_error',
            error_detail: err && err.message ? String(err.message).slice(0, 200) : '',
          });
        }
      } finally {
        if (!controller.signal.aborted) setSubmitEnabled(true);
      }
    }

    function applyAppPrefill() {
      // Consume once per page session — refresh / mode-toggle / post-create
      // re-renders must not re-select the prefilled app (review iter-1).
      if (appPrefillConsumed) return;
      if (!prefill.appId && !prefill.devName) return;
      appPrefillConsumed = true;
      if (prefill.devName && !picker.editedFlag.has(nameInput)) {
        nameInput.value = prefill.devName;
        picker.editedFlag.recordPickerPopulation(nameInput, prefill.devName);
      }
      if (!prefill.appId) return;
      var matched = false;
      for (var i = 0; i < pickerSelect.options.length; i++) {
        if (pickerSelect.options[i].value === prefill.appId) { pickerSelect.selectedIndex = i; matched = true; break; }
      }
      if (!matched) { applyMode('manual'); manualInput.value = prefill.appId; }
    }

    pickerSelect.addEventListener('change', function () {
      var opt = pickerSelect.options[pickerSelect.selectedIndex];
      if (!opt || !opt.value || opt.disabled) return;
      var appName = opt.dataset.appName || opt.textContent || '';
      if (!picker.editedFlag.has(nameInput) && appName) {
        nameInput.value = appName;
        picker.editedFlag.recordPickerPopulation(nameInput, appName);
      }
    });
    pickerRefresh.addEventListener('click', function () {
      loadPicker({ refresh: true }).catch(picker.warnUnlessAbort('app picker refresh'));
    });
    toManual.addEventListener('click', function () {
      if (state.fetchController) { state.fetchController.abort(); state.fetchController = null; }
      applyMode('manual'); setSubmitEnabled(true);
      picker.auditEvent('picker_manual_fallback', { picker_resource: 'application', reason: 'operator_choice' });
    });
    toPicker.addEventListener('click', function () {
      applyMode('picker');
      loadPicker({}).catch(picker.warnUnlessAbort('app picker reload after mode toggle'));
    });

    function readAppId() {
      if (state.mode === 'manual') return (manualInput.value || '').trim();
      return pickerSelect.value || '';
    }

    form.addEventListener('submit', async function (ev) {
      ev.preventDefault();
      clearError(createError);
      var appId = readAppId();
      if (!appId) { showError(createError, 'Please choose an application (or switch to manual entry and type one).'); return; }
      var payload = { application_id: appId, application_name: (nameInput.value || '').trim() };
      try {
        var r = await fetch('/api/applications', {
          method: 'POST', credentials: 'include', headers: jsonHeaders(), body: JSON.stringify(payload),
        });
        if (r.status !== 201 && !r.ok) {
          var b = await r.json().catch(function () { return {}; });
          showError(createError, 'Create failed: ' + (b.error || ('HTTP ' + r.status)));
          return;
        }
        render();
      } catch (e) { showError(createError, 'Create failed: ' + e); }
    });

    // ---- Applications table ----
    var listSection = el('section', { class: 'config-section' });
    listSection.appendChild(el('h2', { text: 'Applications' }));
    var listError = el('div', { class: 'error-banner', hidden: 'hidden' });
    listSection.appendChild(listError);
    var tbody = el('tbody');
    listSection.appendChild(el('table', { class: 'rows' }, [
      el('thead', null, [el('tr', null, [
        el('th', { text: 'Application ID' }),
        el('th', { text: 'Name' }),
        el('th', { text: 'Devices' }),
        el('th', { text: 'Actions' }),
      ])]),
      tbody,
    ]));

    if (apps.length === 0) {
      tbody.appendChild(el('tr', null, [el('td', { colspan: '4', text: 'No applications configured. Use the form above to add one.' })]));
    } else {
      apps.forEach(function (app) {
        var openBtn = el('button', { type: 'button', class: 'btn-open', text: 'Open',
          onclick: function () { navTo('#/app/' + encodeURIComponent(app.application_id)); } });
        var renameBtn = el('button', { type: 'button', class: 'btn-edit', text: 'Rename',
          onclick: function () { renameApp(app.application_id, app.application_name, listError); } });
        var delBtn = el('button', { type: 'button', class: 'btn-delete', text: 'Delete',
          onclick: function () { deleteApp(app.application_id, listError); } });
        tbody.appendChild(el('tr', null, [
          el('td', { text: app.application_id }),
          el('td', { text: app.application_name }),
          el('td', { text: String(app.device_count) }),
          el('td', { class: 'actions' }, [openBtn, renameBtn, delBtn]),
        ]));
      });
    }

    root.replaceChildren(createSection, listSection);

    applyMode(state.mode);
    if (state.mode !== 'manual') {
      loadPicker({}).catch(picker.warnUnlessAbort('initial app picker load'));
    } else {
      applyAppPrefill();
    }
  }

  async function renameApp(appId, currentName, banner) {
    var newName = window.prompt('New application name for ' + appId + ':', currentName);
    if (newName === null || newName === currentName) return;
    try {
      var r = await fetch('/api/applications/' + encodeURIComponent(appId), {
        method: 'PUT', credentials: 'include', headers: jsonHeaders(), body: JSON.stringify({ application_name: newName }),
      });
      if (!r.ok) {
        var b = await r.json().catch(function () { return {}; });
        showError(banner, 'Rename failed: ' + (b.error || ('HTTP ' + r.status))); return;
      }
      render();
    } catch (e) { showError(banner, 'Rename failed: ' + e); }
  }

  async function deleteApp(appId, banner) {
    if (!window.confirm('Delete application ' + appId + '?')) return;
    try {
      var r = await fetch('/api/applications/' + encodeURIComponent(appId), {
        method: 'DELETE', credentials: 'include', headers: jsonHeaders(),
      });
      if (r.status !== 204 && !r.ok) {
        var b = await r.json().catch(function () { return {}; });
        showError(banner, 'Delete failed: ' + (b.error || ('HTTP ' + r.status))); return;
      }
      render();
    } catch (e) { showError(banner, 'Delete failed: ' + e); }
  }

  // =======================================================================
  // VIEW 2 — Devices for one application (with C-2 device + metric pickers).
  // =======================================================================
  async function mountDevices(root, appId, stillCurrent) {
    var prefill = parsePrefill();
    var appName = appId;
    // Provisional breadcrumb (id) before any await, refined to the friendly
    // name below — so a failed load doesn't leave the previous view's trail.
    setBreadcrumb([{ label: 'Applications', href: '#/' }, { label: appId }]);
    // Resolve a friendly app name for the breadcrumb (best-effort).
    var appsRes = await fetchJson('/api/applications');
    if (!stillCurrent()) return;
    if (appsRes.status === 200 && appsRes.body && appsRes.body.applications) {
      var found = appsRes.body.applications.find(function (a) { return a.application_id === appId; });
      if (found) appName = found.application_name;
    }
    setBreadcrumb([{ label: 'Applications', href: '#/' }, { label: appName }]);

    var devUrl = '/api/applications/' + encodeURIComponent(appId) + '/devices';
    var devRes = await fetchJson(devUrl);
    if (!stillCurrent()) return;
    if (devRes.status !== 200 || !devRes.body) {
      throw new Error('GET ' + devUrl + ' failed (status ' + devRes.status + ')');
    }
    var devices = devRes.body.devices || [];

    root.replaceChildren();

    // ---- Device list ----
    var listSection = el('section', { class: 'config-section' });
    listSection.appendChild(el('h2', { text: 'Devices in ' + appName }));
    var listError = el('div', { class: 'error-banner', hidden: 'hidden' });
    listSection.appendChild(listError);
    var tbody = el('tbody');
    listSection.appendChild(el('table', { class: 'rows' }, [
      el('thead', null, [el('tr', null, [
        el('th', { text: 'Device ID' }), el('th', { text: 'Device name' }),
        el('th', { text: 'Metrics' }), el('th', { text: 'Actions' }),
      ])]),
      tbody,
    ]));
    if (devices.length === 0) {
      tbody.appendChild(el('tr', null, [el('td', { colspan: '4', text: '(no devices configured)' })]));
    } else {
      devices.forEach(function (d) {
        var openBtn = el('button', { type: 'button', class: 'btn-open', text: 'Open',
          onclick: function () { navTo('#/app/' + encodeURIComponent(appId) + '/device/' + encodeURIComponent(d.device_id)); } });
        var delBtn = el('button', { type: 'button', class: 'btn-delete', text: 'Delete',
          onclick: function () { deleteDevice(appId, d.device_id, listError); } });
        tbody.appendChild(el('tr', null, [
          el('td', { text: d.device_id }), el('td', { text: d.device_name }),
          el('td', { text: String(d.metric_count) }), el('td', { class: 'actions' }, [openBtn, delBtn]),
        ]));
      });
    }

    // ---- Add-device form (device picker + metric picker) ----
    var addSection = buildAddDeviceSection(appId, prefill);

    root.replaceChildren(addSection, listSection);
  }

  async function deleteDevice(appId, deviceId, banner) {
    if (!window.confirm('Delete device "' + deviceId + '"? Orphaned metric values may persist until the next pruning cycle.')) return;
    var url = '/api/applications/' + encodeURIComponent(appId) + '/devices/' + encodeURIComponent(deviceId);
    try {
      var r = await fetch(url, { method: 'DELETE', credentials: 'include', headers: jsonHeaders() });
      if (r.status !== 204 && !r.ok) {
        var b = await r.json().catch(function () { return {}; });
        showError(banner, 'Delete failed: ' + (b.error || ('HTTP ' + r.status))); return;
      }
      render();
    } catch (e) { showError(banner, 'Delete failed: ' + e); }
  }

  // Add-device form ported from devices-config.js buildApplicationSection,
  // scoped to ONE application. Preserves the device + metric pickers, manual
  // fallbacks, edited-flag, abort discipline, audit, and C-4 deep-link prefill.
  function buildAddDeviceSection(appId, prefill) {
    var section = el('section', { class: 'config-section' });
    section.appendChild(el('h2', { text: 'Add device' }));
    var createErr = el('div', { class: 'error-banner', hidden: 'hidden' });
    var metricContainer = el('div');

    var state = {
      mode: picker.mode.get(DEVICES_PAGE_KEY),
      metricsMode: picker.mode.get(METRICS_PAGE_KEY),
      currentDevEui: '',
      deviceFetchController: null,
      uplinkFetchController: null,
      prefillDevEui: (prefill && prefill.appId === appId) ? (prefill.devEui || '') : '',
      prefillDevName: (prefill && prefill.appId === appId) ? (prefill.devName || '') : '',
      prefillMetricKey: (prefill && prefill.appId === appId) ? (prefill.metricKey || '') : '',
    };

    var form = el('form', { class: 'crud-form' });

    // Device picker
    var devPickerWrap = el('div');
    var devPickerToolbar = el('div', { class: 'picker-toolbar' });
    var devPickerSelect = el('select', { 'aria-label': 'Device from ChirpStack' });
    devPickerSelect.disabled = true;
    devPickerSelect.appendChild(el('option', { value: '', text: 'Loading…' }));
    var devPickerRefresh = el('button', { type: 'button', text: '↻', title: 'Refresh from ChirpStack (cache bypass)' });
    var devToManual = el('a', { role: 'button', tabindex: '0', text: 'Switch to manual entry' });
    devPickerToolbar.appendChild(devPickerSelect); devPickerToolbar.appendChild(devPickerRefresh); devPickerToolbar.appendChild(devToManual);
    devPickerWrap.appendChild(devPickerToolbar);
    var devEuiFootnote = el('div', { class: 'dev-eui-footnote', text: '' });
    devPickerWrap.appendChild(devEuiFootnote);
    var devPickerBanner = el('div', { class: 'picker-fallback-banner' }); devPickerBanner.hidden = true;
    devPickerWrap.appendChild(devPickerBanner);

    var devManualWrap = el('div'); devManualWrap.hidden = true;
    var devManualToolbar = el('div', { class: 'picker-toolbar' });
    var devToPicker = el('a', { role: 'button', tabindex: '0', text: 'Switch to picker' });
    devManualToolbar.appendChild(devToPicker); devManualWrap.appendChild(devManualToolbar);
    var devIdInput = el('input', { type: 'text', placeholder: 'Device ID (DevEUI hex)' });
    devManualWrap.appendChild(el('label', { text: 'Device ID (DevEUI)' }));
    devManualWrap.appendChild(devIdInput);

    form.appendChild(el('label', { text: 'Device from ChirpStack' }));
    form.appendChild(devPickerWrap); form.appendChild(devManualWrap);
    form.appendChild(el('label', { text: 'Device name' }));
    var devNameInput = el('input', { type: 'text', required: 'required' });
    form.appendChild(devNameInput);
    picker.editedFlag.attach(devNameInput);

    // Metric picker
    var metricPickerWrap = el('div');
    var metricPickerToolbar = el('div', { class: 'picker-toolbar' });
    var metricPickerRefresh = el('button', { type: 'button', text: '↻ Refresh metric picker', title: 'Re-fetch the device profile + recent uplinks (cache bypass)' });
    var metricToManual = el('a', { role: 'button', tabindex: '0', text: 'Switch to manual metric entry' });
    metricPickerToolbar.appendChild(metricPickerRefresh); metricPickerToolbar.appendChild(metricToManual);
    metricPickerWrap.appendChild(metricPickerToolbar);
    var metricPickerStatus = el('div', { text: 'Choose a device above first.' });
    metricPickerWrap.appendChild(metricPickerStatus);
    var metricPickerRows = el('div');
    metricPickerWrap.appendChild(metricPickerRows);
    var metricPickerBanner = el('div', { class: 'picker-fallback-banner' }); metricPickerBanner.hidden = true;
    metricPickerWrap.appendChild(metricPickerBanner);

    var metricManualWrap = el('div');
    var metricManualToolbar = el('div', { class: 'picker-toolbar' });
    var metricToPicker = el('a', { role: 'button', tabindex: '0', text: 'Switch to metric picker' });
    metricManualToolbar.appendChild(metricToPicker); metricManualWrap.appendChild(metricManualToolbar);
    metricManualWrap.appendChild(el('h4', { text: 'Metric mappings (manual)' }));
    metricManualWrap.appendChild(metricContainer);
    metricManualWrap.appendChild(el('button', {
      type: 'button', class: 'btn-add-metric', text: '+ Add metric',
      onclick: function () { buildMetricRow(null, metricContainer); },
    }));

    form.appendChild(el('h4', { text: 'Metrics from device profile + recent uplinks (picker)' }));
    form.appendChild(metricPickerWrap); form.appendChild(metricManualWrap);

    var submitBtn = el('button', { type: 'submit', text: 'Create device' });
    form.appendChild(submitBtn); form.appendChild(createErr);

    function applyDeviceMode(mode) {
      state.mode = mode; picker.mode.set(DEVICES_PAGE_KEY, mode);
      if (mode === 'manual') { devPickerWrap.hidden = true; devManualWrap.hidden = false; }
      else { devPickerWrap.hidden = false; devManualWrap.hidden = true; }
    }
    function applyMetricsMode(mode) {
      state.metricsMode = mode; picker.mode.set(METRICS_PAGE_KEY, mode);
      if (mode === 'manual') { metricPickerWrap.hidden = true; metricManualWrap.hidden = false; }
      else { metricPickerWrap.hidden = false; metricManualWrap.hidden = true; }
    }
    function setFormSubmitEnabled(on) {
      if (state.mode === 'manual') { submitBtn.disabled = false; return; }
      submitBtn.disabled = !on;
    }
    function setDevBanner(msg, withRetry) {
      devPickerBanner.replaceChildren();
      if (!msg) { devPickerBanner.hidden = true; return; }
      devPickerBanner.appendChild(document.createTextNode(msg + ' '));
      if (withRetry) {
        devPickerBanner.appendChild(el('button', { type: 'button', text: 'Retry picker',
          onclick: function () { loadDevicePicker({ refresh: true }).catch(picker.warnUnlessAbort('device picker retry')); } }));
      }
      devPickerBanner.hidden = false;
    }
    function setMetricStatus(msg) { metricPickerStatus.textContent = msg || ''; }
    function setMetricBanner(msg) { metricPickerBanner.textContent = msg || ''; metricPickerBanner.hidden = !msg; }

    async function loadDevicePicker(opts) {
      if (state.deviceFetchController) state.deviceFetchController.abort();
      var controller = new AbortController();
      state.deviceFetchController = controller;
      setFormSubmitEnabled(false);
      devPickerSelect.disabled = true;
      devPickerSelect.replaceChildren(el('option', { value: '', text: 'Loading…' }));
      setDevBanner('');
      try {
        try {
          var data = await picker.fetchDevices(appId, { refresh: !!(opts && opts.refresh) });
          if (controller.signal.aborted) return;
          picker.auditEvent('picker_opened', {
            picker_resource: 'device', application_id: appId,
            cache_status: (opts && opts.refresh) ? 'bypassed' : (data.cache_status || 'unknown'),
          });
          if (!data.items || data.items.length === 0) {
            devPickerSelect.replaceChildren(el('option', { value: '', disabled: 'disabled', selected: 'selected', text: '(no devices in ChirpStack)' }));
            applyDeviceMode('manual');
            setDevBanner('No devices found under this application in ChirpStack.', true);
            picker.auditEvent('picker_manual_fallback', { picker_resource: 'device', reason: 'chirpstack_empty' });
            return;
          }
          var items = data.items.slice().sort(byNameCI);
          devPickerSelect.replaceChildren();
          var ph = el('option', { value: '', text: 'Choose a device…' }); ph.disabled = true; ph.selected = true;
          devPickerSelect.appendChild(ph);
          items.forEach(function (item) {
            var opt = el('option', { value: item.dev_eui, text: item.name });
            opt.dataset.devName = item.name; opt.dataset.devEui = item.dev_eui;
            devPickerSelect.appendChild(opt);
          });
          devPickerSelect.disabled = false;
          consumeDevicePrefill();
        } catch (err) {
          if (controller.signal.aborted) return;
          applyDeviceMode('manual');
          setDevBanner('Could not reach ChirpStack — switched to manual entry.', true);
          picker.auditEvent('picker_manual_fallback', {
            picker_resource: 'device',
            reason: err && err.status === 502 ? 'chirpstack_unreachable' : 'chirpstack_error',
            error_detail: err && err.message ? String(err.message).slice(0, 200) : '',
          });
        }
      } finally {
        if (!controller.signal.aborted) setFormSubmitEnabled(true);
      }
    }

    function consumeDevicePrefill() {
      if (!state.prefillDevEui) return;
      var matched = false;
      for (var i = 0; i < devPickerSelect.options.length; i++) {
        if (devPickerSelect.options[i].value === state.prefillDevEui) { devPickerSelect.selectedIndex = i; matched = true; break; }
      }
      if (matched) {
        if (state.prefillDevName && !picker.editedFlag.has(devNameInput)) {
          devNameInput.value = state.prefillDevName;
          picker.editedFlag.recordPickerPopulation(devNameInput, state.prefillDevName);
        }
        devEuiFootnote.textContent = 'DevEUI: ' + state.prefillDevEui;
        if (state.metricsMode === 'picker') {
          loadMetricPicker(state.prefillDevEui).catch(picker.warnUnlessAbort('metric picker after drift prefill'));
        }
      } else {
        applyDeviceMode('manual');
        devIdInput.value = state.prefillDevEui;
        if (state.prefillDevName && !picker.editedFlag.has(devNameInput)) {
          devNameInput.value = state.prefillDevName;
          picker.editedFlag.recordPickerPopulation(devNameInput, state.prefillDevName);
        }
      }
      state.prefillDevEui = ''; state.prefillDevName = '';
    }

    // Story G-1 — the metric picker draws candidates from TWO sources:
    // the device's ChirpStack device-profile measurements (available with
    // no recent traffic) and recently-observed uplink keys. They are
    // merged + de-duplicated by key and each row is tagged with its
    // source (profile / observed / both).
    function appendMetricCandidate(cand, prefillMetricKey) {
      var row = el('div', { class: 'metric-pick-row' });
      var checkboxId = 'mk-' + (_metricCheckboxIdSeq++);
      // data-key / data-inferred are read verbatim by readPickerMetrics().
      var checkbox = el('input', { type: 'checkbox', id: checkboxId, 'data-key': cand.key, 'data-inferred': cand.inferred });
      if (prefillMetricKey && cand.key === prefillMetricKey) checkbox.checked = true;
      var label = el('label', { for: checkboxId, text: cand.key });
      var sourceTag = el('span', { class: 'metric-source-tag', text: cand.source });
      var typeSelect = el('select');
      METRIC_TYPES.forEach(function (t) {
        var opt = el('option', { value: t, text: t });
        if (t === cand.inferred) opt.selected = true;
        typeSelect.appendChild(opt);
      });
      typeSelect.dataset.role = 'wire-type';
      row.appendChild(checkbox); row.appendChild(label); row.appendChild(sourceTag); row.appendChild(typeSelect);
      var detail;
      if (cand.hasSample) {
        var rawSample = JSON.stringify(cand.sample);
        var sampleText = rawSample.length > 200 ? rawSample.slice(0, 200) + '…' : rawSample;
        detail = 'sample: ' + sampleText;
      } else if (cand.kind) {
        detail = 'profile kind: ' + cand.kind;
      } else {
        detail = '';
      }
      if (detail) row.appendChild(el('span', { class: 'sample-cell', text: detail }));
      metricPickerRows.appendChild(row);
    }

    async function loadMetricPicker(devEui, opts) {
      var refresh = !!(opts && opts.refresh);
      state.currentDevEui = devEui;
      metricPickerRows.replaceChildren();
      setMetricBanner('');
      if (state.uplinkFetchController) { state.uplinkFetchController.abort(); state.uplinkFetchController = null; }
      if (!devEui) { setMetricStatus('Choose a device above first.'); return; }
      var controller = new AbortController();
      state.uplinkFetchController = controller;
      setMetricStatus('Loading device profile + recent uplinks…');

      // Fetch both sources; tolerate either failing (degraded mode).
      var measResult = null, upResult = null, measErr = null, upErr = null;
      try { measResult = await picker.fetchMeasurements(devEui, { refresh: refresh }); }
      catch (e) { measErr = e; }
      if (controller.signal.aborted) return;
      try { upResult = await picker.fetchUplinks(devEui, { limit: 10 }); }
      catch (e) { upErr = e; }
      if (controller.signal.aborted) return;

      picker.auditEvent('picker_opened', {
        picker_resource: 'metric', application_id: appId, dev_eui: devEui,
        profile_status: measErr ? 'error' : 'ok', uplink_status: upErr ? 'error' : 'ok',
        cache_status: (measResult && measResult.cache_status) || 'unknown',
      });

      // Merge by key — profile candidates first (declarative source), then
      // fold in observed uplink keys, upgrading shared keys to 'both'.
      var byKey = {};
      ((measResult && measResult.items) || []).forEach(function (m) {
        byKey[m.key] = { key: m.key, inferred: m.metric_type, source: 'profile', kind: m.kind, hasSample: false };
      });
      ((upResult && upResult.observed_keys) || []).forEach(function (k) {
        if (byKey[k.key]) {
          byKey[k.key].source = 'profile + observed';
          byKey[k.key].sample = k.sample_value; byKey[k.key].hasSample = true;
        } else {
          byKey[k.key] = { key: k.key, inferred: k.wire_type, source: 'observed', sample: k.sample_value, hasSample: true };
        }
      });
      var cands = Object.keys(byKey).map(function (k) { return byKey[k]; })
        .sort(function (a, b) { return a.key < b.key ? -1 : (a.key > b.key ? 1 : 0); });

      if (cands.length === 0) {
        if (measErr || upErr) {
          setMetricStatus('Could not fetch the device profile or recent uplinks.');
          setMetricBanner('You can still add metrics manually below.');
        } else {
          setMetricStatus('No device-profile measurements and no recent uplinks for this device. Add metrics manually below.');
        }
        applyMetricsMode('manual');
        picker.auditEvent('picker_manual_fallback', {
          picker_resource: 'metric',
          reason: (measErr || upErr) ? ((measErr && measErr.status === 502) || (upErr && upErr.status === 502) ? 'chirpstack_unreachable' : 'chirpstack_error') : 'no_candidates',
        });
        return;
      }

      // Partial-degraded notice when one source failed but the other gave data.
      var notes = [];
      if (measErr) notes.push('device profile unavailable');
      if (upErr) notes.push('recent uplinks unavailable');
      if (notes.length) setMetricBanner('Partial data — ' + notes.join(', ') + '. Showing what is available; add others manually if needed.');

      setMetricStatus('Tick metric keys to include; override wire type per row if needed.');
      var prefillMetricKey = state.prefillMetricKey || ''; state.prefillMetricKey = '';
      cands.forEach(function (cand) { appendMetricCandidate(cand, prefillMetricKey); });
    }

    function readPickerMetrics() {
      var rows = metricPickerRows.querySelectorAll('.metric-pick-row');
      var result = [];
      rows.forEach(function (row) {
        var checkbox = row.querySelector('input[type="checkbox"]');
        if (!checkbox || !checkbox.checked) return;
        var key = checkbox.dataset.key;
        var inferred = checkbox.dataset.inferred || '';
        var chosen = row.querySelector('select[data-role="wire-type"]').value;
        result.push({
          metric_name: key, chirpstack_metric_name: key, metric_type: chosen,
          picker_metadata: { inferred_type: inferred, operator_chosen_type: chosen },
        });
      });
      return result;
    }

    devPickerSelect.addEventListener('change', function () {
      var opt = devPickerSelect.options[devPickerSelect.selectedIndex];
      if (!opt || !opt.value || opt.disabled) {
        devEuiFootnote.textContent = ''; state.currentDevEui = '';
        metricPickerRows.replaceChildren(); setMetricStatus('Choose a device above first.'); return;
      }
      var devName = opt.dataset.devName || opt.textContent || '';
      var devEui = opt.dataset.devEui || opt.value || '';
      if (!picker.editedFlag.has(devNameInput) && devName) {
        devNameInput.value = devName; picker.editedFlag.recordPickerPopulation(devNameInput, devName);
      }
      devEuiFootnote.textContent = devEui ? 'DevEUI: ' + devEui : '';
      if (state.metricsMode === 'picker') loadMetricPicker(devEui).catch(picker.warnUnlessAbort('metric picker after device pick'));
    });
    devPickerRefresh.addEventListener('click', function () { loadDevicePicker({ refresh: true }).catch(picker.warnUnlessAbort('device picker refresh')); });
    devToManual.addEventListener('click', function () {
      if (state.deviceFetchController) { state.deviceFetchController.abort(); state.deviceFetchController = null; }
      applyDeviceMode('manual'); setFormSubmitEnabled(true);
      picker.auditEvent('picker_manual_fallback', { picker_resource: 'device', reason: 'operator_choice' });
    });
    devToPicker.addEventListener('click', function () { applyDeviceMode('picker'); loadDevicePicker({}).catch(picker.warnUnlessAbort('device picker reload after mode toggle')); });
    metricPickerRefresh.addEventListener('click', function () {
      if (!state.currentDevEui) {
        var opt = devPickerSelect.options[devPickerSelect.selectedIndex];
        var eui = opt && opt.dataset ? opt.dataset.devEui : '';
        if (eui) { loadMetricPicker(eui, { refresh: true }).catch(picker.warnUnlessAbort('metric picker refresh')); }
        else { setMetricStatus(state.mode === 'manual' ? 'Type a DevEUI in the device-id field first.' : 'Select a device first.'); }
        return;
      }
      loadMetricPicker(state.currentDevEui, { refresh: true }).catch(picker.warnUnlessAbort('metric picker refresh'));
    });
    metricToManual.addEventListener('click', function () {
      applyMetricsMode('manual');
      picker.auditEvent('picker_manual_fallback', { picker_resource: 'uplink', reason: 'operator_choice' });
    });
    metricToPicker.addEventListener('click', function () {
      applyMetricsMode('picker');
      if (state.currentDevEui) loadMetricPicker(state.currentDevEui).catch(picker.warnUnlessAbort('metric picker reload after mode toggle'));
    });

    form.addEventListener('submit', function (ev) {
      ev.preventDefault();
      clearError(createErr);
      var deviceId = '';
      var deviceName = String(devNameInput.value || '').trim();
      if (state.mode === 'manual') deviceId = String(devIdInput.value || '').trim();
      else {
        var opt = devPickerSelect.options[devPickerSelect.selectedIndex];
        deviceId = opt ? (opt.dataset.devEui || opt.value || '') : '';
      }
      if (!deviceId) { showError(createErr, 'Choose a device from the picker, or switch to manual entry and type a DevEUI.'); return; }
      if (!deviceName) { showError(createErr, 'Device name is required.'); return; }
      var metrics = state.metricsMode === 'picker' ? readPickerMetrics() : readMetricsFromContainer(metricContainer);
      var payload = { device_id: deviceId, device_name: deviceName, read_metric_list: metrics };
      var url = '/api/applications/' + encodeURIComponent(appId) + '/devices';
      fetch(url, { method: 'POST', credentials: 'include', headers: jsonHeaders(), body: JSON.stringify(payload) })
        .then(async function (res) {
          if (res.status !== 201) {
            var b = await res.json().catch(function () { return null; });
            showError(createErr, (b && b.error) ? b.error : ('POST failed: ' + res.status)); return;
          }
          render();
        }).catch(function (err) { showError(createErr, 'Network error: ' + err.message); });
    });

    section.appendChild(form);

    applyDeviceMode(state.mode);
    applyMetricsMode(state.metricsMode);
    if (state.mode === 'picker') loadDevicePicker({}).catch(picker.warnUnlessAbort('initial device picker load'));
    return section;
  }

  // =======================================================================
  // VIEW 3 — Device detail: Metrics + Commands.
  // =======================================================================
  async function mountDeviceDetail(root, appId, deviceId, stillCurrent) {
    // Provisional breadcrumb (ids) before any await; refined to friendly names
    // below — a failed load won't leave the previous view's trail (iter-1).
    setBreadcrumb([
      { label: 'Applications', href: '#/' },
      { label: appId, href: '#/app/' + encodeURIComponent(appId) },
      { label: deviceId },
    ]);
    var devUrl = '/api/applications/' + encodeURIComponent(appId) + '/devices/' + encodeURIComponent(deviceId);
    var res = await fetchJson(devUrl);
    if (!stillCurrent()) return;
    if (res.status !== 200 || !res.body) {
      throw new Error('GET ' + devUrl + ' failed (status ' + res.status + ')');
    }
    var dev = res.body;

    // Best-effort app name for the breadcrumb.
    var appName = appId;
    var appsRes = await fetchJson('/api/applications');
    if (!stillCurrent()) return;
    if (appsRes.status === 200 && appsRes.body && appsRes.body.applications) {
      var found = appsRes.body.applications.find(function (a) { return a.application_id === appId; });
      if (found) appName = found.application_name;
    }
    setBreadcrumb([
      { label: 'Applications', href: '#/' },
      { label: appName, href: '#/app/' + encodeURIComponent(appId) },
      { label: dev.device_name || deviceId },
    ]);

    root.replaceChildren();

    // ---- Metrics panel (device name + read_metric mappings) ----
    var metricsSection = el('section', { class: 'config-section' });
    metricsSection.appendChild(el('h2', { text: 'Metrics' }));
    var metricsError = el('div', { class: 'error-banner', hidden: 'hidden' });
    var mForm = el('form', { class: 'crud-form' });
    mForm.appendChild(el('label', { text: 'Device name' }));
    var devNameInput = el('input', { type: 'text', required: 'required', value: dev.device_name || '' });
    mForm.appendChild(devNameInput);
    // Story G-3 (#132): optional per-device OPC UA stale threshold. Empty =
    // use the global [opcua].stale_threshold_seconds default (120 s).
    mForm.appendChild(el('label', { text: 'Stale threshold (seconds) — optional, overrides the global default' }));
    var staleInput = el('input', {
      type: 'number', min: '1', max: '86400',
      placeholder: 'leave empty to use the global default',
      value: (dev.stale_threshold_seconds === null || dev.stale_threshold_seconds === undefined)
        ? '' : String(dev.stale_threshold_seconds),
    });
    mForm.appendChild(staleInput);
    var staleHelp = fieldHelp('device.stale_threshold_seconds', staleInput);
    if (staleHelp) mForm.appendChild(staleHelp);
    mForm.appendChild(el('h3', { text: 'Metric mappings' }));
    var metricContainer = el('div');
    (dev.read_metric_list || []).forEach(function (m) { buildMetricRow(m, metricContainer); });
    mForm.appendChild(metricContainer);
    mForm.appendChild(el('button', { type: 'button', class: 'btn-add-metric', text: '+ Add metric',
      onclick: function () { buildMetricRow(null, metricContainer); } }));
    mForm.appendChild(el('button', { type: 'submit', text: 'Save changes' }));
    mForm.appendChild(metricsError);
    metricsSection.appendChild(mForm);

    mForm.addEventListener('submit', async function (ev) {
      ev.preventDefault();
      clearError(metricsError);
      // Story G-3 (#132): empty → null (global default). Otherwise require a
      // whole number in (0, 86400]; reject decimals/out-of-range/non-numeric
      // client-side rather than silently truncating or turning bad input into
      // null. (Number() accepts exponent forms like "1e3" → 1000, which is a
      // valid value.) The server re-validates the band regardless.
      var staleRaw = (staleInput.value || '').trim();
      var staleVal = null;
      if (staleRaw !== '') {
        var n = Number(staleRaw);
        if (!Number.isInteger(n) || n < 1 || n > 86400) {
          showError(metricsError, 'Stale threshold must be a whole number of seconds between 1 and 86400 (or leave it empty to use the global default).');
          return;
        }
        staleVal = n;
      }
      var payload = {
        device_name: (devNameInput.value || '').trim(),
        read_metric_list: readMetricsFromContainer(metricContainer),
        // null clears the override (back to the global default); PUT-replace.
        stale_threshold_seconds: staleVal,
      };
      try {
        var r = await fetch(devUrl, { method: 'PUT', credentials: 'include', headers: jsonHeaders(), body: JSON.stringify(payload) });
        if (r.status !== 200) {
          var b = await r.json().catch(function () { return null; });
          showError(metricsError, (b && b.error) || ('PUT failed: ' + r.status)); return;
        }
        render();
      } catch (e) { showError(metricsError, 'Network error: ' + e.message); }
    });

    // ---- Commands panel ----
    var cmdSection = el('section', { class: 'config-section' });
    cmdSection.appendChild(el('h2', { text: 'Commands' }));
    var cmdError = el('div', { class: 'error-banner', hidden: 'hidden' });
    cmdSection.appendChild(cmdError);
    var cmdTableWrap = el('div', { text: 'Loading commands…' });
    cmdSection.appendChild(cmdTableWrap);
    // Command mutations refresh ONLY the commands table — never the whole
    // device-detail view — so unsaved edits in the Metrics panel survive
    // (review iter-1: render() here discarded in-progress metric edits).
    var refreshCmds = function () { return refreshCommandsTable(appId, deviceId, cmdTableWrap, cmdError, refreshCmds); };
    cmdSection.appendChild(buildCommandCreateForm(appId, deviceId, cmdError, refreshCmds));

    root.replaceChildren(metricsSection, cmdSection);
    await refreshCmds();
  }

  var CMD_CLASS_OPTIONS = [
    { value: '', label: '(none — generic raw byte)' },
    { value: 'valve', label: 'valve' },
  ];

  function buildCommandCreateForm(appId, deviceId, banner, refreshCmds) {
    var form = el('form', { class: 'crud-form' });
    form.appendChild(el('h3', { text: 'Create command' }));
    var idInput = el('input', { type: 'number', min: '1', required: 'required' });
    var nameInput = el('input', { type: 'text', required: 'required' });
    var portInput = el('input', { type: 'number', min: '1', max: '223', required: 'required' });
    var confirmedInput = el('input', { type: 'checkbox' });
    var classSelect = el('select');
    CMD_CLASS_OPTIONS.forEach(function (o) { classSelect.appendChild(el('option', { value: o.value, text: o.label })); });
    // Story G-2 (#142): contextual help on each command field.
    form.appendChild(el('label', { text: 'command_id' })); appendHelp(form, 'command.command_id', idInput); form.appendChild(idInput);
    form.appendChild(el('label', { text: 'command_name' })); appendHelp(form, 'command.command_name', nameInput); form.appendChild(nameInput);
    form.appendChild(el('label', { text: 'command_port (LoRaWAN f_port, 1–223)' })); appendHelp(form, 'command.command_port', portInput); form.appendChild(portInput);
    form.appendChild(el('label', null, [confirmedInput, document.createTextNode(' Confirmed downlink')])); appendHelp(form, 'command.command_confirmed', confirmedInput);
    form.appendChild(el('label', { text: 'command_class' })); appendHelp(form, 'command.command_class', classSelect); form.appendChild(classSelect);
    form.appendChild(el('button', { type: 'submit', class: 'btn-add', text: 'Create command' }));

    form.addEventListener('submit', async function (ev) {
      ev.preventDefault();
      clearError(banner);
      var payload = {
        command_id: parseInt(idInput.value, 10),
        command_name: String(nameInput.value || '').trim(),
        command_port: parseInt(portInput.value, 10),
        command_confirmed: confirmedInput.checked,
      };
      var cc = String(classSelect.value || '').trim();
      if (cc) payload.command_class = cc;
      var url = '/api/applications/' + encodeURIComponent(appId) + '/devices/' + encodeURIComponent(deviceId) + '/commands';
      try {
        var r = await fetch(url, { method: 'POST', credentials: 'include', headers: jsonHeaders(), body: JSON.stringify(payload) });
        if (r.status !== 201 && !r.ok) {
          var b = await r.json().catch(function () { return {}; });
          showError(banner, 'Create failed: ' + (b.error || ('HTTP ' + r.status))); return;
        }
        idInput.value = ''; nameInput.value = ''; portInput.value = ''; confirmedInput.checked = false; classSelect.value = '';
        refreshCmds();
      } catch (e) { showError(banner, 'Create failed: ' + (e.message || e)); }
    });
    return form;
  }

  async function refreshCommandsTable(appId, deviceId, container, banner, refreshCmds) {
    container.replaceChildren(document.createTextNode('Loading commands…'));
    var url = '/api/applications/' + encodeURIComponent(appId) + '/devices/' + encodeURIComponent(deviceId) + '/commands';
    var res = await fetchJson(url);
    if (res.status !== 200 || !res.body) {
      container.replaceChildren();
      showError(banner, 'Failed to load commands (status ' + res.status + ')'); return;
    }
    var commands = res.body.commands || [];
    container.replaceChildren();
    if (commands.length === 0) {
      container.appendChild(el('p', null, [el('em', { text: 'No commands configured for this device.' })]));
      return;
    }
    var tbody = el('tbody');
    commands.forEach(function (c) {
      var editBtn = el('button', { type: 'button', class: 'btn-edit', text: 'Edit',
        onclick: function () { openCommandEdit(appId, deviceId, c, banner, refreshCmds); } });
      var delBtn = el('button', { type: 'button', class: 'btn-delete', text: 'Delete',
        onclick: function () { deleteCommand(appId, deviceId, c.command_id, banner, refreshCmds); } });
      tbody.appendChild(el('tr', null, [
        el('td', { text: String(c.command_id) }),
        el('td', { text: c.command_name }),
        el('td', { text: String(c.command_port) }),
        el('td', { text: c.command_confirmed ? 'true' : 'false' }),
        el('td', { text: c.command_class ? c.command_class : '—' }),
        el('td', { class: 'actions' }, [editBtn, delBtn]),
      ]));
    });
    container.appendChild(el('table', { class: 'commands' }, [
      el('thead', null, [el('tr', null, [
        el('th', { text: 'command_id' }), el('th', { text: 'command_name' }), el('th', { text: 'command_port' }),
        el('th', { text: 'command_confirmed' }), el('th', { text: 'command_class' }), el('th', { text: 'Actions' }),
      ])]),
      tbody,
    ]));
  }

  function openCommandEdit(appId, deviceId, cmd, banner, refreshCmds) {
    var dialog = el('dialog', { class: 'modal' });
    var errBanner = el('div', { class: 'error-banner', hidden: 'hidden' });
    var nameInput = el('input', { type: 'text', required: 'required', value: cmd.command_name || '' });
    var portInput = el('input', { type: 'number', min: '1', max: '223', required: 'required', value: String(cmd.command_port) });
    var confirmedInput = el('input', { type: 'checkbox' });
    confirmedInput.checked = !!cmd.command_confirmed;
    var classSelect = el('select');
    CMD_CLASS_OPTIONS.forEach(function (o) {
      var opt = el('option', { value: o.value, text: o.label });
      if (o.value === (cmd.command_class || '')) opt.selected = true;
      classSelect.appendChild(opt);
    });
    var form = el('form', { class: 'crud-form' });
    form.appendChild(el('h2', { text: 'Edit command ' + cmd.command_id }));
    form.appendChild(el('label', { text: 'command_name' })); form.appendChild(nameInput);
    form.appendChild(el('label', { text: 'command_port (1–223)' })); form.appendChild(portInput);
    form.appendChild(el('label', null, [confirmedInput, document.createTextNode(' Confirmed downlink')]));
    form.appendChild(el('label', { text: 'command_class' })); form.appendChild(classSelect);
    form.appendChild(errBanner);
    var cancelBtn = el('button', { type: 'button', text: 'Cancel' });
    form.appendChild(el('div', { class: 'actions' }, [el('button', { type: 'submit', text: 'Save changes' }), cancelBtn]));
    var content = el('div', { class: 'modal-content' }, [form]);
    dialog.appendChild(content);
    document.body.appendChild(dialog);

    // Remove the dialog from the DOM whenever it closes — including the native
    // Escape-key close, which bypasses our Cancel/Save buttons (review iter-1:
    // orphaned dialogs accumulated on Escape and survived hash navigation).
    dialog.addEventListener('close', function () { dialog.remove(); });
    function close() {
      if (typeof dialog.close === 'function') { try { dialog.close(); } catch (_) { dialog.removeAttribute('open'); } }
      else dialog.removeAttribute('open');
      dialog.remove();
    }
    cancelBtn.addEventListener('click', close);
    form.addEventListener('submit', async function (ev) {
      ev.preventDefault();
      clearError(errBanner);
      var payload = {
        command_name: (nameInput.value || '').trim(),
        command_port: parseInt(portInput.value, 10),
        command_confirmed: confirmedInput.checked,
        command_class: (String(classSelect.value || '').trim() || null),
      };
      var url = '/api/applications/' + encodeURIComponent(appId) + '/devices/' + encodeURIComponent(deviceId) + '/commands/' + cmd.command_id;
      try {
        var r = await fetch(url, { method: 'PUT', credentials: 'include', headers: jsonHeaders(), body: JSON.stringify(payload) });
        if (r.status !== 200 && !r.ok) {
          var b = await r.json().catch(function () { return {}; });
          showError(errBanner, 'Edit failed: ' + (b.error || ('HTTP ' + r.status))); return;
        }
        close(); refreshCmds();
      } catch (e) { showError(errBanner, 'Edit failed: ' + (e.message || e)); }
    });
    if (typeof dialog.showModal === 'function') { try { dialog.showModal(); } catch (_) { dialog.setAttribute('open', 'open'); } }
    else dialog.setAttribute('open', 'open');
  }

  async function deleteCommand(appId, deviceId, commandId, banner, refreshCmds) {
    if (!window.confirm('Delete command ' + commandId + '?')) return;
    var url = '/api/applications/' + encodeURIComponent(appId) + '/devices/' + encodeURIComponent(deviceId) + '/commands/' + commandId;
    try {
      var r = await fetch(url, { method: 'DELETE', credentials: 'include', headers: jsonHeaders() });
      if (r.status !== 204 && !r.ok) {
        var b = await r.json().catch(function () { return {}; });
        showError(banner, 'Delete failed: ' + (b.error || ('HTTP ' + r.status))); return;
      }
      refreshCmds();
    } catch (e) { showError(banner, 'Delete failed: ' + (e.message || e)); }
  }

  // -----------------------------------------------------------------------
  // Shared metric-row builders (ported from devices-config.js).
  // -----------------------------------------------------------------------
  function buildMetricRow(metric, container) {
    var typeSelect = el('select', { name: 'metric_type' });
    METRIC_TYPES.forEach(function (t) {
      var opt = el('option', { value: t, text: t });
      if (metric && metric.metric_type === t) opt.selected = true;
      typeSelect.appendChild(opt);
    });
    var nameInput = el('input', { type: 'text', name: 'metric_name', value: (metric && metric.metric_name) || '', required: 'required' });
    var csNameInput = el('input', { type: 'text', name: 'chirpstack_metric_name', value: (metric && metric.chirpstack_metric_name) || '', required: 'required' });
    var unitInput = el('input', { type: 'text', name: 'metric_unit', value: (metric && metric.metric_unit) || '' });
    // Story G-2 (#142): each metric field carries a contextual-help affordance.
    var row = el('div', { class: 'metric-row' }, [
      el('div', null, [el('label', { text: 'metric_name' }), fieldHelp('metric.metric_name', nameInput), nameInput]),
      el('div', null, [el('label', { text: 'chirpstack_metric_name' }), fieldHelp('metric.chirpstack_metric_name', csNameInput), csNameInput]),
      el('div', null, [el('label', { text: 'metric_type' }), fieldHelp('metric.metric_type', typeSelect), typeSelect]),
      el('div', null, [el('label', { text: 'metric_unit (optional)' }), fieldHelp('metric.metric_unit', unitInput), unitInput]),
      el('button', { type: 'button', class: 'btn-remove-metric', text: '×', title: 'Remove this metric',
        onclick: function () { row.remove(); } }),
    ]);
    container.appendChild(row);
  }

  function readMetricsFromContainer(container) {
    var result = [];
    container.querySelectorAll('.metric-row').forEach(function (row) {
      var obj = {
        metric_name: row.querySelector('input[name="metric_name"]').value.trim(),
        chirpstack_metric_name: row.querySelector('input[name="chirpstack_metric_name"]').value.trim(),
        metric_type: row.querySelector('select[name="metric_type"]').value,
      };
      var unit = row.querySelector('input[name="metric_unit"]').value.trim();
      if (unit.length > 0) obj.metric_unit = unit;
      result.push(obj);
    });
    return result;
  }

  function byNameCI(a, b) {
    var ax = (a.name || '').toLowerCase();
    var bx = (b.name || '').toLowerCase();
    return ax < bx ? -1 : ax > bx ? 1 : 0;
  }

  // -----------------------------------------------------------------------
  // Boot.
  // -----------------------------------------------------------------------
  window.addEventListener('hashchange', render);
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', render);
  } else {
    render();
  }
})();
