// SPDX-License-Identifier: MIT OR Apache-2.0
// (c) [2024] Guy Corbaz
//
// Story 9-4: vanilla JS controller for /applications.html.
// Story C-2: extended with name-driven inventory picker fed by
// /api/inventory/applications + manual-entry fallback toggle.
//
// All mutating fetches set Content-Type: application/json and
// rely on the browser to attach Origin (which the gateway's CSRF
// middleware checks against [web].allowed_origins).

(function () {
  var PAGE_KEY = 'applications';
  var picker = window.opcgwPicker;

  var tbody = document.getElementById('applications-tbody');
  var listError = document.getElementById('list-error');
  var createForm = document.getElementById('create-form');
  var createError = document.getElementById('create-error');

  // Picker DOM
  var pickerWrap = document.getElementById('application-picker-wrap');
  var manualWrap = document.getElementById('application-manual-wrap');
  var pickerSelect = document.getElementById('application-picker');
  var pickerBanner = document.getElementById('application-picker-banner');
  var pickerRefresh = document.getElementById('application-picker-refresh');
  var modeToManual = document.getElementById('application-mode-to-manual');
  var modeToPicker = document.getElementById('application-mode-to-picker');
  var manualInput = document.getElementById('new-application-id');
  var nameInput = document.getElementById('new-application-name');

  function showError(el, msg) {
    el.textContent = msg;
    el.hidden = false;
  }
  function clearError(el) {
    el.textContent = '';
    el.hidden = true;
  }

  function setPickerBanner(msg) {
    pickerBanner.textContent = '';
    if (!msg) {
      pickerBanner.hidden = true;
      return;
    }
    pickerBanner.textContent = msg + ' ';
    var retry = document.createElement('button');
    retry.type = 'button';
    retry.textContent = 'Retry picker';
    retry.addEventListener('click', function () {
      loadPicker({ refresh: true });
    });
    pickerBanner.appendChild(retry);
    pickerBanner.hidden = false;
  }

  // ----------------------------------------------------------------
  // Application list view (existing 9-4 surface, unchanged).
  // ----------------------------------------------------------------
  async function fetchApplications() {
    clearError(listError);
    try {
      const r = await fetch('/api/applications', { credentials: 'include' });
      if (!r.ok) {
        showError(listError, 'Failed to load applications: HTTP ' + r.status);
        return;
      }
      const data = await r.json();
      renderRows(data.applications || []);
    } catch (e) {
      showError(listError, 'Failed to load applications: ' + e);
    }
  }

  function renderRows(apps) {
    tbody.innerHTML = '';
    if (apps.length === 0) {
      const tr = document.createElement('tr');
      tr.innerHTML =
        '<td colspan="4" style="text-align:center; padding:1.5rem; color:#777;">' +
          'No applications configured. Use the form above to add one.' +
        '</td>';
      tbody.appendChild(tr);
      return;
    }
    apps.forEach(function (app) {
      const tr = document.createElement('tr');
      tr.innerHTML =
        '<td class="app-id">' + picker.escapeHtml(app.application_id) + '</td>' +
        '<td class="app-name">' + picker.escapeHtml(app.application_name) + '</td>' +
        '<td class="app-dev-count">' + app.device_count + '</td>' +
        '<td class="actions">' +
          '<button class="btn-edit" data-id="' + picker.escapeHtml(app.application_id) + '" data-name="' + picker.escapeHtml(app.application_name) + '">Edit</button>' +
          '<button class="btn-delete" data-id="' + picker.escapeHtml(app.application_id) + '">Delete</button>' +
        '</td>';
      tbody.appendChild(tr);
    });
    tbody.querySelectorAll('.btn-edit').forEach(function (btn) {
      btn.addEventListener('click', function () { onEdit(btn.dataset.id, btn.dataset.name); });
    });
    tbody.querySelectorAll('.btn-delete').forEach(function (btn) {
      btn.addEventListener('click', function () { onDelete(btn.dataset.id); });
    });
  }

  async function onEdit(applicationId, currentName) {
    const newName = window.prompt(
      'New application name for ' + applicationId + ':',
      currentName,
    );
    if (newName === null || newName === currentName) return;
    try {
      const r = await fetch('/api/applications/' + encodeURIComponent(applicationId), {
        method: 'PUT',
        credentials: 'include',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ application_name: newName }),
      });
      if (!r.ok) {
        const body = await r.json().catch(function () { return {}; });
        showError(listError, 'Edit failed: ' + (body.error || ('HTTP ' + r.status)));
        return;
      }
      fetchApplications();
    } catch (e) {
      showError(listError, 'Edit failed: ' + e);
    }
  }

  async function onDelete(applicationId) {
    if (!window.confirm('Delete application ' + applicationId + '?')) return;
    try {
      const r = await fetch('/api/applications/' + encodeURIComponent(applicationId), {
        method: 'DELETE',
        credentials: 'include',
        headers: { 'Content-Type': 'application/json' },
      });
      if (r.status !== 204 && !r.ok) {
        const body = await r.json().catch(function () { return {}; });
        showError(listError, 'Delete failed: ' + (body.error || ('HTTP ' + r.status)));
        return;
      }
      fetchApplications();
    } catch (e) {
      showError(listError, 'Delete failed: ' + e);
    }
  }

  // ----------------------------------------------------------------
  // Story C-2 picker mode (AC#1 / #2 / #3 / #16 / #18).
  // ----------------------------------------------------------------
  function applyMode(mode) {
    if (mode === 'manual') {
      pickerWrap.hidden = true;
      manualWrap.hidden = false;
      manualInput.required = true;
    } else {
      pickerWrap.hidden = false;
      manualWrap.hidden = true;
      manualInput.required = false;
    }
    picker.mode.set(PAGE_KEY, mode);
  }

  async function loadPicker(opts) {
    pickerSelect.disabled = true;
    pickerSelect.innerHTML = '<option>Loading…</option>';
    setPickerBanner(null);
    var cacheStatus = 'unknown';
    try {
      var data = await picker.fetchApplications({ refresh: !!(opts && opts.refresh) });
      cacheStatus = data.cache_status || 'unknown';
      picker.auditEvent('picker_opened', {
        picker_resource: 'application',
        cache_status: cacheStatus,
      });
      if (!data.items || data.items.length === 0) {
        // AC#1: empty inventory → pre-flip to manual + show context.
        pickerSelect.innerHTML = '<option>(no applications in ChirpStack)</option>';
        applyMode('manual');
        setPickerBanner('No applications found in ChirpStack for this tenant — type one manually or create one in ChirpStack first.');
        picker.auditEvent('picker_manual_fallback', {
          picker_resource: 'application',
          reason: 'chirpstack_empty',
        });
        return;
      }
      // Client-side alphabetical sort (defensive — API already sorts).
      var items = data.items.slice().sort(function (a, b) {
        var ax = (a.name || '').toLowerCase();
        var bx = (b.name || '').toLowerCase();
        return ax < bx ? -1 : ax > bx ? 1 : 0;
      });
      pickerSelect.innerHTML = '';
      var placeholder = document.createElement('option');
      placeholder.value = '';
      placeholder.textContent = 'Choose an application…';
      placeholder.disabled = true;
      placeholder.selected = true;
      pickerSelect.appendChild(placeholder);
      items.forEach(function (item) {
        var opt = document.createElement('option');
        opt.value = item.id;
        opt.textContent = item.name;
        opt.dataset.appName = item.name;
        pickerSelect.appendChild(opt);
      });
      pickerSelect.disabled = false;
    } catch (err) {
      // AC#1 / AC#2: auto-fallback on 502 or any other error.
      var reason = err && err.status === 502 ? 'chirpstack_unreachable' : 'chirpstack_error';
      applyMode('manual');
      setPickerBanner('Could not reach ChirpStack — switched to manual entry.');
      picker.auditEvent('picker_manual_fallback', {
        picker_resource: 'application',
        reason: reason,
        error_detail: err && err.message ? String(err.message).slice(0, 200) : '',
      });
    }
  }

  // ----------------------------------------------------------------
  // Form submit: assemble payload from whichever mode is active.
  // ----------------------------------------------------------------
  function readApplicationIdFromActiveMode() {
    if (manualWrap.hidden === false) {
      return (manualInput.value || '').trim();
    }
    var sel = pickerSelect;
    return sel.value || '';
  }

  function setupPickerEventListeners() {
    // Re-populate name field on picker selection (AC#3 — only if
    // operator has not edited the field).
    pickerSelect.addEventListener('change', function () {
      var opt = pickerSelect.options[pickerSelect.selectedIndex];
      var appName = opt ? (opt.dataset.appName || opt.textContent || '') : '';
      if (!picker.editedFlag.has(nameInput) && appName) {
        nameInput.value = appName;
      }
    });
    pickerRefresh.addEventListener('click', function () {
      picker.auditEvent('picker_opened', {
        picker_resource: 'application',
        cache_status: 'bypassed',
      });
      loadPicker({ refresh: true });
    });
    modeToManual.addEventListener('click', function () {
      applyMode('manual');
      picker.auditEvent('picker_manual_fallback', {
        picker_resource: 'application',
        reason: 'operator_choice',
      });
    });
    modeToPicker.addEventListener('click', function () {
      applyMode('picker');
      loadPicker({});
    });
    picker.editedFlag.attach(nameInput);
  }

  createForm.addEventListener('submit', async function (event) {
    event.preventDefault();
    clearError(createError);
    var applicationId = readApplicationIdFromActiveMode();
    if (!applicationId) {
      showError(createError, 'Please choose an application (or switch to manual entry and type one).');
      return;
    }
    var payload = {
      application_id: applicationId,
      application_name: (nameInput.value || '').trim(),
    };
    try {
      var r = await fetch('/api/applications', {
        method: 'POST',
        credentials: 'include',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload),
      });
      if (r.status !== 201 && !r.ok) {
        var body = await r.json().catch(function () { return {}; });
        showError(createError, 'Create failed: ' + (body.error || ('HTTP ' + r.status)));
        return;
      }
      manualInput.value = '';
      nameInput.value = '';
      picker.editedFlag.reset(nameInput);
      fetchApplications();
      // Refetch on next form open: cache may serve a hit (AC#17, #20).
      if (manualWrap.hidden) loadPicker({});
    } catch (e) {
      showError(createError, 'Create failed: ' + e);
    }
  });

  document.addEventListener('DOMContentLoaded', function () {
    setupPickerEventListeners();
    applyMode(picker.mode.get(PAGE_KEY));
    if (manualWrap.hidden) loadPicker({});
    fetchApplications();
  });
})();
