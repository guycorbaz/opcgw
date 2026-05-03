// opcgw dashboard — Story 9-2 (FR38).
//
// Polls /api/status on load + every 10 s. No SPA framework, no build
// step. Browser caches Basic-auth credentials per realm; 401 from
// /api/status is treated as "credentials revoked" — operator gets an
// inline banner and can reload to re-prompt.
//
// Review iter-1 patches:
//   - E2: in-flight guard prevents overlapping fetches when network is
//         slow (previous setInterval would compound and race render).
//   - E5: per-call AbortController cancels the prior fetch when a new
//         one starts (operator click-spam can no longer DoS).
//   - E7: chirpstack_available branches explicitly on `=== false` for
//         "Unavailable"; missing/null/non-bool renders as "Unknown".
//   - E8: Content-Type sniff before resp.json() — proxy login pages
//         no longer crash the dashboard with cryptic SyntaxError.
//   - B9: network-error banner is generic — err.message no longer
//         interpolated into the DOM (consistent with NFR7 server side).
//   - E13: formatRelative + absolute timestamp share one parse pass
//          so "—" vs raw-ISO mismatch can no longer surface.

(function () {
  "use strict";

  var REFRESH_INTERVAL_MS = 10000;
  var ENDPOINT = "/api/status";

  var els = {
    chirpstack: document.getElementById("chirpstack-status"),
    lastPollRel: document.getElementById("last-poll-relative"),
    lastPollTime: document.getElementById("last-poll-time"),
    errorCount: document.getElementById("error-count"),
    appCount: document.getElementById("application-count"),
    devCount: document.getElementById("device-count"),
    uptime: document.getElementById("uptime"),
    lastRefresh: document.getElementById("last-refresh"),
    errorBanner: document.getElementById("error-banner"),
    refreshButton: document.getElementById("refresh-now"),
  };

  var numberFormatter = new Intl.NumberFormat();
  var dateFormatter = new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "medium",
  });

  // Review iter-1 E2 + E5: in-flight guard + AbortController. Holds
  // the controller so a new request can cancel the previous one
  // (operator click-spam / setInterval race no longer compound).
  //
  // Review iter-2 M2: feature-detect AbortController so older browsers
  // (pre-2018: Safari < 11.1, Edge < 16, Chrome < 66) don't get a
  // synchronous ReferenceError that breaks the dashboard silently.
  // When AbortController is unavailable the `inflightToken` falls back
  // to a plain object identity — the M1 stale-render guard still
  // works, only the abort-on-supersede behaviour degrades gracefully.
  var ABORT_SUPPORTED = typeof AbortController !== "undefined";
  var inflightToken = null;

  function setBadge(el, label, kind) {
    el.textContent = label;
    el.classList.remove("badge-available", "badge-unavailable", "badge-unknown");
    el.classList.add("badge-" + kind);
  }

  function showError(message) {
    els.errorBanner.textContent = message;
    els.errorBanner.classList.remove("hidden");
  }

  function clearError() {
    els.errorBanner.textContent = "";
    els.errorBanner.classList.add("hidden");
  }

  // Parse an ISO string once and return both representations the
  // dashboard needs. Avoids the iter-1 E13 quirk where formatRelative
  // returned "—" for unparseable but the absolute tile rendered the
  // raw string — operator saw inconsistent values for the same field.
  function parseTimestamp(iso) {
    if (iso === null || iso === undefined) {
      return { ok: false, reason: "never" };
    }
    var d = new Date(iso);
    var t = d.getTime();
    if (isNaN(t)) {
      return { ok: false, reason: "unparseable" };
    }
    return { ok: true, date: d, ms: t };
  }

  function formatRelative(parsed) {
    if (!parsed.ok) {
      return parsed.reason === "never" ? "Never polled" : "—";
    }
    var deltaSecs = Math.max(0, Math.floor((Date.now() - parsed.ms) / 1000));
    if (deltaSecs < 60) {
      return deltaSecs + " s ago";
    }
    if (deltaSecs < 3600) {
      return Math.floor(deltaSecs / 60) + " min ago";
    }
    if (deltaSecs < 86400) {
      return Math.floor(deltaSecs / 3600) + " h ago";
    }
    return Math.floor(deltaSecs / 86400) + " d ago";
  }

  function formatUptime(secs) {
    if (typeof secs !== "number" || secs < 0) {
      return "—";
    }
    var d = Math.floor(secs / 86400);
    var h = Math.floor((secs % 86400) / 3600);
    var m = Math.floor((secs % 3600) / 60);
    var s = secs % 60;
    if (d > 0) {
      return d + "d " + h + "h";
    }
    if (h > 0) {
      return h + "h " + m + "m";
    }
    if (m > 0) {
      return m + "m " + s + "s";
    }
    return s + "s";
  }

  function render(data) {
    // Review iter-1 E7: explicit `=== false` branch. Anything that's
    // not strictly true OR strictly false (missing field, null,
    // unexpected type) renders as Unknown — the failure mode "field
    // missing" no longer collapses with "ChirpStack down".
    if (data.chirpstack_available === true) {
      setBadge(els.chirpstack, "Available", "available");
    } else if (data.chirpstack_available === false) {
      setBadge(els.chirpstack, "Unavailable", "unavailable");
    } else {
      setBadge(els.chirpstack, "Unknown", "unknown");
    }

    var parsed = parseTimestamp(data.last_poll_time);
    els.lastPollRel.textContent = formatRelative(parsed);
    if (!parsed.ok) {
      els.lastPollTime.setAttribute("datetime", "");
      els.lastPollTime.textContent =
        parsed.reason === "never" ? "—" : "Invalid timestamp";
    } else {
      els.lastPollTime.setAttribute("datetime", data.last_poll_time);
      els.lastPollTime.textContent = dateFormatter.format(parsed.date);
    }

    els.errorCount.textContent = numberFormatter.format(data.error_count || 0);
    els.appCount.textContent = numberFormatter.format(data.application_count || 0);
    els.devCount.textContent = numberFormatter.format(data.device_count || 0);
    els.uptime.textContent = formatUptime(data.uptime_secs);
    els.lastRefresh.textContent = dateFormatter.format(new Date());
  }

  function fetchStatus() {
    // Review iter-1 E5: cancel any in-flight request so a slow-network
    // backlog doesn't pile up when refresh-now is clicked or the
    // setInterval fires while the previous fetch is still pending.
    //
    // Review iter-2 M2: feature-detect; degrade to no-abort + token
    // identity-only on browsers that pre-date AbortController.
    var controller = ABORT_SUPPORTED ? new AbortController() : null;
    if (ABORT_SUPPORTED && inflightToken !== null) {
      // The previous token IS an AbortController on this branch.
      inflightToken.abort();
    }
    var thisCallToken = controller !== null ? controller : {};
    inflightToken = thisCallToken;

    var fetchOpts = {
      cache: "no-store",
      credentials: "same-origin",
    };
    if (controller !== null) {
      fetchOpts.signal = controller.signal;
    }

    fetch(ENDPOINT, fetchOpts)
      .then(function (resp) {
        if (resp.status === 401) {
          showError(
            "Session expired or credentials no longer accepted. Please reload the page."
          );
          return null;
        }
        if (!resp.ok) {
          showError(
            "Status unavailable (HTTP " + resp.status + "). Last successful refresh shown above."
          );
          return null;
        }
        // Review iter-1 E8: a reverse proxy returning a 200 + HTML
        // login page would otherwise crash render() via JSON.parse.
        // Sniff Content-Type before parsing so the operator sees a
        // useful banner instead of "Unexpected token <".
        var ct = resp.headers.get("content-type") || "";
        if (ct.indexOf("application/json") === -1) {
          showError(
            "Status unavailable (upstream returned non-JSON; check proxy / auth gateway configuration)."
          );
          return null;
        }
        clearError();
        return resp.json();
      })
      .then(function (data) {
        // Review iter-2 M1: stale-render guard. After AbortController
        // signals, the prior call's `.then(resp => …)` chain may
        // already have a resolved JSON value sitting in a microtask
        // queue that runs AFTER the new call's render. Without this
        // guard, the stale data overwrites the fresh data and the
        // operator sees flicker / regression. Drop any data whose
        // owning call has been superseded.
        if (data && inflightToken === thisCallToken) {
          render(data);
        }
      })
      .catch(function (err) {
        // AbortError is expected on rapid-refresh — silently swallow.
        if (err && err.name === "AbortError") {
          return;
        }
        // Review iter-2 M1: only show the error banner if THIS call
        // is still the live one. Otherwise a stale network error
        // from an aborted call would clobber the new call's
        // `clearError()`.
        if (inflightToken !== thisCallToken) {
          return;
        }
        // Review iter-1 B9: generic banner — err.message can carry
        // operator-noise (CORS / SSL / DNS specifics) and is
        // inconsistent with the server-side NFR7 stance on hiding
        // internals. Keep the surface clean.
        showError(
          "Status unavailable (network error). Check the gateway connection."
        );
      })
      .finally(function () {
        // Only clear the in-flight token if we are still the current
        // call (otherwise a later call has already taken ownership
        // and we must not stomp on it).
        if (inflightToken === thisCallToken) {
          inflightToken = null;
        }
      });
  }

  els.refreshButton.addEventListener("click", fetchStatus);

  // Initial fetch + periodic refresh.
  fetchStatus();
  setInterval(fetchStatus, REFRESH_INTERVAL_MS);
})();
