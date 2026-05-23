// SPDX-License-Identifier: MIT OR Apache-2.0
// (c) [2024] Guy Corbaz
//
// Story C-2: shared client-side module for inventory pickers.
// Vanilla JS, no SPA framework, no build step. Loaded as a sibling
// `<script src="/inventory-picker.js">` BEFORE `applications.js` and
// `devices-config.js` so the page controllers can call into the
// `window.opcgwPicker` namespace exported below.
//
// Surface (all read-mostly; no global mutable state beyond
// localStorage):
//   - opcgwPicker.fetchApplications({ refresh? })       -> {items, count, cache_status, fetched_at}
//   - opcgwPicker.fetchDevices(application_id, { refresh? })
//   - opcgwPicker.fetchUplinks(dev_eui, { limit?, refresh? })
//   - opcgwPicker.auditEvent(eventName, fields)         -> Promise<void>
//   - opcgwPicker.mode.get(pageKey)                     -> "picker" | "manual"
//   - opcgwPicker.mode.set(pageKey, mode)
//   - opcgwPicker.editedFlag.attach(inputEl)            -> flips a hidden flag on first keystroke
//   - opcgwPicker.editedFlag.has(inputEl)
//   - opcgwPicker.editedFlag.reset(inputEl)
//   - opcgwPicker.escapeHtml(s)
//
// The module is intentionally light on framework abstractions: it
// provides primitives, NOT prefabricated <select> renderers, so each
// page can shape the picker DOM to fit its existing layout (the
// application-add form is a simple single-select; the device-add form
// lives in a per-application section; the metric picker is a
// multi-checkbox).

(function () {
  'use strict';

  // -------------------------------------------------------------------
  // HTML-escape helper. Mirrors the legacy implementation in
  // applications.js so both pages agree on the safe-rendering surface.
  // -------------------------------------------------------------------
  function escapeHtml(s) {
    return String(s)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;')
      .replace(/`/g, '&#96;');
  }

  // -------------------------------------------------------------------
  // Fetch wrappers — thin layers over /api/inventory/{...} that surface
  // the cache_status field so callers can drive `auditEvent`
  // ("picker_opened" with the cache_status as a field) per AC#12.
  //
  // The caller decides what to do with a 502 (manual fallback per
  // AC#1/#5). These helpers do NOT auto-fallback — they throw an
  // Error with .status set so the caller can pattern-match without
  // re-parsing the response.
  // -------------------------------------------------------------------
  async function fetchJson(url) {
    const r = await fetch(url, { credentials: 'include' });
    if (!r.ok) {
      const err = new Error('HTTP ' + r.status + ' for ' + url);
      err.status = r.status;
      // Best-effort detail extraction; failures are non-fatal here.
      try {
        err.body = await r.json();
      } catch (_) {
        err.body = null;
      }
      throw err;
    }
    return r.json();
  }

  function buildUrl(base, params) {
    const usp = new URLSearchParams();
    Object.keys(params || {}).forEach(function (k) {
      const v = params[k];
      if (v === undefined || v === null || v === false) return;
      if (v === true) {
        usp.set(k, 'true');
      } else {
        usp.set(k, String(v));
      }
    });
    const q = usp.toString();
    return q.length === 0 ? base : base + '?' + q;
  }

  async function fetchApplications(opts) {
    const params = {};
    if (opts && opts.refresh) params.refresh = true;
    return fetchJson(buildUrl('/api/inventory/applications', params));
  }

  async function fetchDevices(applicationId, opts) {
    if (!applicationId) {
      throw new Error('fetchDevices requires application_id');
    }
    const params = { application_id: applicationId };
    if (opts && opts.refresh) params.refresh = true;
    return fetchJson(buildUrl('/api/inventory/devices', params));
  }

  async function fetchUplinks(devEui, opts) {
    if (!devEui) {
      throw new Error('fetchUplinks requires dev_eui');
    }
    const params = { dev_eui: devEui };
    if (opts && typeof opts.limit === 'number') params.limit = opts.limit;
    return fetchJson(buildUrl('/api/inventory/uplinks', params));
  }

  // -------------------------------------------------------------------
  // Audit-event helper — POST /api/audit/picker-event.
  //
  // Network failures are intentionally swallowed: if the operator's
  // browser cannot reach the audit endpoint, that is itself an
  // operator-visible problem (the picker would have failed anyway),
  // and we do not want the picker's UX to surface a duplicate alert.
  // The function returns the response status code on success or null
  // on failure so callers can log/console.warn if they wish.
  // -------------------------------------------------------------------
  async function auditEvent(eventName, fields) {
    try {
      const r = await fetch('/api/audit/picker-event', {
        method: 'POST',
        credentials: 'include',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ event: eventName, fields: fields || {} }),
      });
      return r.status;
    } catch (_e) {
      return null;
    }
  }

  // -------------------------------------------------------------------
  // Mode persistence (AC#2, #5, #18) — localStorage-backed picker-vs-
  // manual toggle. One key per pageKey ("applications" / "devices" /
  // "metrics") so cross-page leakage is impossible.
  // -------------------------------------------------------------------
  function modeStorageKey(pageKey) {
    return 'opcgw.picker.' + pageKey + '.mode';
  }

  function getMode(pageKey) {
    try {
      const stored = window.localStorage.getItem(modeStorageKey(pageKey));
      if (stored === 'manual' || stored === 'picker') return stored;
    } catch (_e) {
      // localStorage may throw under strict privacy modes; default
      // to picker on inaccessible storage.
    }
    return 'picker';
  }

  function setMode(pageKey, mode) {
    if (mode !== 'manual' && mode !== 'picker') {
      throw new Error('mode must be "manual" or "picker"; got ' + mode);
    }
    try {
      window.localStorage.setItem(modeStorageKey(pageKey), mode);
    } catch (_e) {
      // Best-effort persistence; non-fatal.
    }
  }

  // -------------------------------------------------------------------
  // Edited-flag heuristic (AC#3) — once the operator types into the
  // pre-filled name field, we do not re-populate it on subsequent
  // picker-selection changes. The flag is stored on the DOM element
  // via a custom dataset attribute so it survives across rerenders
  // that re-query the same element by id.
  // -------------------------------------------------------------------
  function attachEditedFlag(inputEl) {
    if (!inputEl) return;
    // Use a closure-once handler so we do not stack listeners if the
    // helper is called more than once on the same element.
    if (inputEl.dataset.opcgwEditedAttached === '1') return;
    inputEl.dataset.opcgwEditedAttached = '1';
    inputEl.addEventListener('keydown', function () {
      inputEl.dataset.opcgwEdited = '1';
    });
  }

  function hasEditedFlag(inputEl) {
    return !!(inputEl && inputEl.dataset && inputEl.dataset.opcgwEdited === '1');
  }

  function resetEditedFlag(inputEl) {
    if (inputEl && inputEl.dataset) {
      delete inputEl.dataset.opcgwEdited;
    }
  }

  // -------------------------------------------------------------------
  // Export the surface under a single global namespace so the legacy
  // page controllers can call into it without having to manage
  // explicit module wiring.
  // -------------------------------------------------------------------
  window.opcgwPicker = {
    escapeHtml: escapeHtml,
    fetchApplications: fetchApplications,
    fetchDevices: fetchDevices,
    fetchUplinks: fetchUplinks,
    auditEvent: auditEvent,
    mode: {
      get: getMode,
      set: setMode,
    },
    editedFlag: {
      attach: attachEditedFlag,
      has: hasEditedFlag,
      reset: resetEditedFlag,
    },
  };
})();
