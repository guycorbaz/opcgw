// opcgw dashboard — Story 9-2 (FR38).
//
// Polls /api/status on load + every 10 s. No SPA framework, no build
// step. Browser caches Basic-auth credentials per realm; 401 from
// /api/status is treated as "credentials revoked" — operator gets an
// inline banner and can reload to re-prompt.

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

  function formatRelative(iso) {
    if (iso === null || iso === undefined) {
      return "Never polled";
    }
    var then = new Date(iso).getTime();
    if (isNaN(then)) {
      return "—";
    }
    var deltaSecs = Math.max(0, Math.floor((Date.now() - then) / 1000));
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
    if (data.chirpstack_available === true) {
      setBadge(els.chirpstack, "Available", "available");
    } else {
      setBadge(els.chirpstack, "Unavailable", "unavailable");
    }

    els.lastPollRel.textContent = formatRelative(data.last_poll_time);
    if (data.last_poll_time === null || data.last_poll_time === undefined) {
      els.lastPollTime.setAttribute("datetime", "");
      els.lastPollTime.textContent = "—";
    } else {
      els.lastPollTime.setAttribute("datetime", data.last_poll_time);
      var d = new Date(data.last_poll_time);
      els.lastPollTime.textContent = isNaN(d.getTime())
        ? data.last_poll_time
        : dateFormatter.format(d);
    }

    els.errorCount.textContent = numberFormatter.format(data.error_count || 0);
    els.appCount.textContent = numberFormatter.format(data.application_count || 0);
    els.devCount.textContent = numberFormatter.format(data.device_count || 0);
    els.uptime.textContent = formatUptime(data.uptime_secs);
    els.lastRefresh.textContent = dateFormatter.format(new Date());
  }

  function fetchStatus() {
    fetch(ENDPOINT, { cache: "no-store", credentials: "same-origin" })
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
        clearError();
        return resp.json();
      })
      .then(function (data) {
        if (data) {
          render(data);
        }
      })
      .catch(function (err) {
        showError("Status unavailable (network error: " + err.message + ").");
      });
  }

  els.refreshButton.addEventListener("click", fetchStatus);

  // Initial fetch + periodic refresh.
  fetchStatus();
  setInterval(fetchStatus, REFRESH_INTERVAL_MS);
})();
