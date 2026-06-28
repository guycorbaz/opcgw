// Story G-4 (#127): the dashboard error drill-down view.
//
// Fetches GET /api/errors and renders the recent error-event feed
// newest-first (time, category, device/application, sanitized message).
// Vanilla, no build step. Reachable from the dashboard "Errors" tile.

(function () {
  'use strict';

  var REFRESH_MS = 15000;
  var els = {};
  var timer = null;

  function escapeText(s) {
    return s == null ? '' : String(s);
  }

  function setBanner(msg) {
    if (!els.banner) return;
    if (msg) {
      els.banner.textContent = msg;
      els.banner.classList.remove('hidden');
    } else {
      els.banner.textContent = '';
      els.banner.classList.add('hidden');
    }
  }

  function fmtTime(ts) {
    // ts is an RFC3339 string; show local time, fall back to the raw value.
    var d = new Date(ts);
    return isNaN(d.getTime()) ? escapeText(ts) : d.toLocaleString();
  }

  function deviceOrApp(ev) {
    if (ev.device_id) return 'device: ' + ev.device_id;
    if (ev.application_id) return 'app: ' + ev.application_id;
    return '—';
  }

  function render(items) {
    var tbody = els.tbody;
    tbody.replaceChildren();
    if (!items || items.length === 0) {
      els.status.textContent = 'No recent errors recorded. 🎉';
      els.status.hidden = false;
      els.table.hidden = true;
      return;
    }
    items.forEach(function (ev) {
      var tr = document.createElement('tr');
      [fmtTime(ev.ts), escapeText(ev.category), deviceOrApp(ev), escapeText(ev.message)]
        .forEach(function (text) {
          var td = document.createElement('td');
          td.textContent = text;
          tr.appendChild(td);
        });
      tbody.appendChild(tr);
    });
    els.status.hidden = true;
    els.table.hidden = false;
  }

  async function load() {
    try {
      var r = await fetch('/api/errors?limit=200', { credentials: 'include' });
      if (!r.ok) {
        setBanner('Could not load errors (HTTP ' + r.status + ').');
        return;
      }
      var data = await r.json();
      setBanner('');
      render(data.items || []);
      if (els.lastRefresh) els.lastRefresh.textContent = new Date().toLocaleTimeString();
    } catch (e) {
      setBanner('Network error loading errors: ' + (e && e.message ? e.message : e));
    }
  }

  document.addEventListener('DOMContentLoaded', function () {
    els.status = document.getElementById('errors-status');
    els.table = document.getElementById('errors-table');
    els.tbody = document.getElementById('errors-tbody');
    els.banner = document.getElementById('error-banner');
    els.lastRefresh = document.getElementById('last-refresh');
    var refreshBtn = document.getElementById('refresh-now');
    if (refreshBtn) refreshBtn.addEventListener('click', load);
    load();
    timer = setInterval(load, REFRESH_MS);
    window.addEventListener('beforeunload', function () { if (timer) clearInterval(timer); });
  });
})();
