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
  var submitButton = createForm.querySelector('button[type="submit"]');

  // Iter-1 review HIGH-5 + HIGH-7 state:
  // - `pickerState.mode` is the authoritative source of truth for
  //   whether the form submit reads from the picker `<select>` or the
  //   manual `<input>`. NEVER read this from DOM hidden attributes;
  //   the CSS/HTML hidden state and this in-memory flag are kept in
  //   sync by applyMode(), but the in-memory flag is what reflects
  //   the operator's intent.
  // - `pickerState.fetchController` holds the AbortController for the
  //   most recent inventory fetch so a rapid second click aborts the
  //   first request before starting the second (prevents stale-wins
  //   races).
  var pickerState = {
    mode: 'picker',
    fetchController: null,
  };

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
    // Iter-1 review HIGH-5: pickerState.mode is the authoritative
    // source of truth — the DOM `hidden` flags below are just the
    // visual presentation. readApplicationIdFromActiveMode() must
    // consult pickerState.mode, NOT the DOM hidden state.
    pickerState.mode = mode;
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

  function setSubmitEnabled(enabled) {
    // Iter-1 review HIGH-7: disable submit while picker fetch in
    // flight so the operator cannot accidentally post "Loading…"
    // (or the placeholder's empty-value) as application_id.
    if (submitButton) submitButton.disabled = !enabled;
  }

  async function loadPicker(opts) {
    // Iter-1 review HIGH-7: abort any in-flight fetch before starting
    // a new one. Without this, rapid refresh-clicks can produce
    // out-of-order resolutions where the older fetch overwrites the
    // newer picker contents.
    if (pickerState.fetchController) {
      pickerState.fetchController.abort();
    }
    var controller = new AbortController();
    pickerState.fetchController = controller;
    setSubmitEnabled(false);
    pickerSelect.disabled = true;
    // Iter-1 review HIGH-7: Loading option carries value="" so the
    // submit handler's empty-string check rejects mid-load submits
    // (defence-in-depth alongside the submit-disable above).
    pickerSelect.innerHTML = '<option value="">Loading…</option>';
    setPickerBanner(null);
    var cacheStatus = 'unknown';
    // Iter-2 review HIGH-1: `finally` safety net so the submit button
    // re-enables on EVERY exit path — including the aborted-fetch
    // early-return and any synchronous throw. Pre-fix, both old AND
    // new fetches returning via the abort-check path could leave the
    // submit button wedged disabled until a manual toggle.
    try {
    try {
      var data = await picker.fetchApplications({ refresh: !!(opts && opts.refresh) });
      // Iter-1 review HIGH-7: if a newer fetch aborted us, drop the
      // stale resolution silently.
      if (controller.signal.aborted) return;
      cacheStatus = data.cache_status || 'unknown';
      picker.auditEvent('picker_opened', {
        picker_resource: 'application',
        cache_status: cacheStatus,
      });
      if (!data.items || data.items.length === 0) {
        // AC#1: empty inventory → pre-flip to manual + show context.
        // Iter-2 review LOW-10: explicit value="" so a programmatic
        // submit before the operator interacts never posts the
        // placeholder textContent as application_id.
        pickerSelect.innerHTML = '<option value="" disabled selected>(no applications in ChirpStack)</option>';
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
      // Iter-1 review HIGH-7: drop stale aborted fetches silently.
      if (controller.signal.aborted) return;
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
    } finally {
      // Iter-2 review HIGH-1: re-enable submit on every exit path
      // (success / error / abort early-return). Only valid for THIS
      // fetch — if a newer fetch aborted us, the newer fetch's own
      // finally will own the final state.
      if (!controller.signal.aborted) {
        setSubmitEnabled(true);
      }
    }
  }

  // ----------------------------------------------------------------
  // Form submit: assemble payload from whichever mode is active.
  // ----------------------------------------------------------------
  function readApplicationIdFromActiveMode() {
    // Iter-1 review HIGH-5: read from pickerState.mode, NOT
    // manualWrap.hidden. The DOM hidden attribute can desync from the
    // logical mode (CSS override, future style refactor); the
    // in-memory mode tracked by applyMode() is the source of truth.
    if (pickerState.mode === 'manual') {
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
      // Iter-3 review MED — guard against the disabled placeholder
      // ("Choose an application…") being re-selected programmatically
      // via selectedIndex=0: pre-iter-3, the change handler would
      // write the placeholder's textContent into nameInput AND record
      // it as the picker-populated baseline, leaking "Choose…" into
      // the create-form submission. Skip pre-fill when the option is
      // missing, has no value, or is disabled.
      if (!opt || !opt.value || opt.disabled) return;
      var appName = opt.dataset.appName || opt.textContent || '';
      if (!picker.editedFlag.has(nameInput) && appName) {
        nameInput.value = appName;
        // Iter-2 review HIGH-4: record the picker-populated value so
        // a browser-autofill restoration of the same value cannot
        // false-positive the edited flag on the operator's behalf.
        picker.editedFlag.recordPickerPopulation(nameInput, appName);
      }
    });
    pickerRefresh.addEventListener('click', function () {
      // Iter-1 review LOW-1 fix: drop the pre-fetch picker_opened
      // emit — loadPicker() emits its own picker_opened on success
      // with the server-provided cache_status, which will be the
      // "bypassed" / "miss" value reflecting the actual ?refresh=true
      // bypass result. Pre-fix produced double-emits.
      loadPicker({ refresh: true });
    });
    modeToManual.addEventListener('click', function () {
      // Iter-3 review HIGH-2: abort any in-flight picker fetch BEFORE
      // applyMode flips the visible UI. Without this, the stale fetch
      // continues to mutate the now-hidden pickerSelect DOM and emits
      // a `picker_opened` audit for state that's no longer
      // operator-visible.
      if (pickerState.fetchController) {
        pickerState.fetchController.abort();
        pickerState.fetchController = null;
      }
      applyMode('manual');
      // Iter-2 review HIGH-3: switching to manual must re-enable the
      // submit button even if a picker fetch was in flight (otherwise
      // the operator is stuck in manual mode with a disabled submit
      // until the now-irrelevant fetch resolves).
      setSubmitEnabled(true);
      picker.auditEvent('picker_manual_fallback', {
        picker_resource: 'application',
        reason: 'operator_choice',
      });
    });
    modeToPicker.addEventListener('click', function () {
      applyMode('picker');
      // Iter-3 review MED: filter AbortError out of the warn (abort
      // is the EXPECTED outcome on rapid mode-toggle); other failures
      // still surface to DevTools.
      loadPicker({}).catch(picker.warnUnlessAbort('picker reload after mode toggle'));
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
      // Iter-1 review HIGH-5: read pickerState.mode, not DOM hidden.
      // Iter-3 review MED: AbortError-filtered warning.
      if (pickerState.mode !== 'manual') {
        loadPicker({}).catch(picker.warnUnlessAbort('picker reload after create'));
      }
    } catch (e) {
      showError(createError, 'Create failed: ' + e);
    }
  });

  // Story C-4 (AC#8): honour deep-link prefill query params from the
  // drift view. `prefill_app_id` selects the matching picker option
  // (or falls back to manual mode if the id isn't in the picker's
  // option set); `prefill_name` pre-fills the application_name input
  // unless the operator has already edited it. The drift view never
  // mutates state directly — this prefill is the deep-link contract
  // it relies on.
  function applyPrefillFromUrl() {
    var params;
    try {
      params = new URLSearchParams(window.location.search || '');
    } catch (e) {
      return;
    }
    var prefillId = params.get('prefill_app_id') || '';
    var prefillName = params.get('prefill_name') || '';
    if (!prefillId && !prefillName) return;

    if (prefillName && !picker.editedFlag.has(nameInput)) {
      nameInput.value = prefillName;
      picker.editedFlag.recordPickerPopulation(nameInput, prefillName);
    }
    if (!prefillId) return;

    // Try to select the matching picker option. If the picker isn't
    // populated yet (still loading) or the id isn't an option, fall
    // back to manual mode with the id pre-filled.
    var matched = false;
    if (pickerSelect && pickerSelect.options) {
      for (var i = 0; i < pickerSelect.options.length; i++) {
        if (pickerSelect.options[i].value === prefillId) {
          pickerSelect.selectedIndex = i;
          matched = true;
          break;
        }
      }
    }
    if (!matched) {
      applyMode('manual');
      if (manualInput) manualInput.value = prefillId;
    }
    // Scroll the create form into view so the operator lands on it.
    var form = document.getElementById('create-form');
    if (form && typeof form.scrollIntoView === 'function') {
      form.scrollIntoView({ behavior: 'smooth', block: 'center' });
    }
  }

  document.addEventListener('DOMContentLoaded', function () {
    setupPickerEventListeners();
    applyMode(picker.mode.get(PAGE_KEY));
    // Iter-1 review HIGH-5: read pickerState.mode, not DOM hidden.
    // Iter-3 review MED: AbortError-filtered warning.
    if (pickerState.mode !== 'manual') {
      loadPicker({})
        .then(applyPrefillFromUrl)
        .catch(picker.warnUnlessAbort('initial picker load'));
    } else {
      // Manual mode: no picker options to wait for; apply prefill now.
      applyPrefillFromUrl();
    }
    fetchApplications();
  });
})();
