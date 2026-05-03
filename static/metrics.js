// opcgw live metrics — Story 9-3 (FR37).
//
// Polls /api/devices on load + every 10 s. Renders the application-
// grouped grid of per-device metric values with a staleness colour
// per row computed from (as_of - timestamp) vs the two threshold
// fields the server ships in the JSON response.
//
// Defensive fetch path mirrors static/dashboard.js (Story 9-2 iter-2):
//   - in-flight guard + AbortController feature-detect (M2)
//   - stale-render guard (M1) — superseded calls can't stomp the DOM
//   - Content-Type sniff before resp.json() (E8)
//   - generic error banner (B9) — err.message not interpolated
//   - shared parseTimestamp() pass between formatRelative + abs tile
//
// Story 9-3 carry-forward LOWs from the 9-2 iter-2 review acknowledged
// here: L1 (Content-Type case sensitivity — applies symmetrically;
// kept loose to match the dashboard pattern; future shared-helper
// extraction can fix both at once).

(function () {
  "use strict";

  var REFRESH_INTERVAL_MS = 10000;
  var ENDPOINT = "/api/devices";

  var els = {
    grid: document.getElementById("grid-container"),
    lastRefresh: document.getElementById("last-refresh"),
    errorBanner: document.getElementById("error-banner"),
    refreshButton: document.getElementById("refresh-now"),
  };

  var dateFormatter = new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "medium",
  });

  // M2: feature-detect AbortController.
  var ABORT_SUPPORTED = typeof AbortController !== "undefined";
  var inflightToken = null;

  function showError(message) {
    els.errorBanner.textContent = message;
    els.errorBanner.classList.remove("hidden");
  }

  function clearError() {
    els.errorBanner.textContent = "";
    els.errorBanner.classList.add("hidden");
  }

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

  function formatRelative(parsed, asOfMs) {
    if (!parsed.ok) {
      return parsed.reason === "never" ? "Never reported" : "—";
    }
    var deltaSecs = Math.max(0, Math.floor((asOfMs - parsed.ms) / 1000));
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

  // Returns one of: "good" / "uncertain" / "bad" / "missing".
  function statusFor(metric, asOfMs, staleThresholdSecs, badThresholdSecs) {
    if (metric.value === null || metric.value === undefined) {
      return "missing";
    }
    var parsed = parseTimestamp(metric.timestamp);
    if (!parsed.ok) {
      return "missing";
    }
    var ageSecs = Math.max(0, Math.floor((asOfMs - parsed.ms) / 1000));
    if (ageSecs >= badThresholdSecs) {
      return "bad";
    }
    if (ageSecs >= staleThresholdSecs) {
      return "uncertain";
    }
    return "good";
  }

  // DOM element factory — small helper to avoid heavy innerHTML
  // strings (which would also be an XSS surface if we ever templated
  // operator-controlled strings into them).
  function el(tag, attrs, children) {
    var node = document.createElement(tag);
    if (attrs) {
      for (var k in attrs) {
        if (Object.prototype.hasOwnProperty.call(attrs, k)) {
          if (k === "class") {
            node.className = attrs[k];
          } else if (k === "data-label") {
            node.setAttribute("data-label", attrs[k]);
          } else if (k === "datetime") {
            node.setAttribute("datetime", attrs[k]);
          } else {
            node.setAttribute(k, attrs[k]);
          }
        }
      }
    }
    if (children) {
      for (var i = 0; i < children.length; i++) {
        var c = children[i];
        if (typeof c === "string") {
          node.appendChild(document.createTextNode(c));
        } else if (c) {
          node.appendChild(c);
        }
      }
    }
    return node;
  }

  function renderMetricRow(metric, asOfMs, staleThresholdSecs, badThresholdSecs) {
    var status = statusFor(metric, asOfMs, staleThresholdSecs, badThresholdSecs);
    var parsed = parseTimestamp(metric.timestamp);
    var valueText = metric.value === null || metric.value === undefined ? "—" : metric.value;

    var statusLabel = status.charAt(0).toUpperCase() + status.slice(1);

    var lastUpdateCell;
    if (parsed.ok) {
      lastUpdateCell = el("td", { "data-label": "Last update" }, [
        formatRelative(parsed, asOfMs) + " (",
        el("time", { datetime: metric.timestamp }, [dateFormatter.format(parsed.date)]),
        ")",
      ]);
    } else {
      lastUpdateCell = el("td", { "data-label": "Last update" }, [
        parsed.reason === "never" ? "Never reported" : "—",
      ]);
    }

    return el("tr", { class: "row-" + status }, [
      el("td", { "data-label": "Metric" }, [metric.metric_name]),
      el("td", { "data-label": "Value", class: "metric-value" }, [valueText]),
      el("td", { "data-label": "Type" }, [metric.data_type || "?"]),
      lastUpdateCell,
      el("td", { "data-label": "Status", class: "metric-status" }, [statusLabel]),
    ]);
  }

  function renderDevice(device, asOfMs, staleThresholdSecs, badThresholdSecs) {
    var rows = [
      el("tr", null, [
        el("th", null, ["Metric"]),
        el("th", null, ["Value"]),
        el("th", null, ["Type"]),
        el("th", null, ["Last update"]),
        el("th", null, ["Status"]),
      ]),
    ];
    for (var i = 0; i < device.metrics.length; i++) {
      rows.push(
        renderMetricRow(device.metrics[i], asOfMs, staleThresholdSecs, badThresholdSecs)
      );
    }

    var bodyChildren = [];
    if (device.metrics.length === 0) {
      bodyChildren.push(
        el("p", { class: "empty-application" }, ["No metrics configured for this device."])
      );
    } else {
      bodyChildren.push(
        el("table", { class: "metrics-table" }, [
          el("thead", null, [rows[0]]),
          el("tbody", null, rows.slice(1)),
        ])
      );
    }

    return el("section", { class: "device" }, [
      el("h3", null, [
        device.device_name,
        el("span", { class: "device-id" }, ["(" + device.device_id + ")"]),
      ]),
    ].concat(bodyChildren));
  }

  function renderApplication(app, asOfMs, staleThresholdSecs, badThresholdSecs) {
    var children = [
      el("h2", null, [
        app.application_name,
        el("span", { class: "device-count-badge" }, [app.devices.length + " devices"]),
      ]),
    ];
    if (app.devices.length === 0) {
      children.push(
        el("p", { class: "empty-application" }, ["No devices configured for this application."])
      );
    } else {
      for (var i = 0; i < app.devices.length; i++) {
        children.push(
          renderDevice(app.devices[i], asOfMs, staleThresholdSecs, badThresholdSecs)
        );
      }
    }
    return el("section", { class: "application" }, children);
  }

  function render(data) {
    var asOfParsed = parseTimestamp(data.as_of);
    var asOfMs = asOfParsed.ok ? asOfParsed.ms : Date.now();
    // Review iter-1 L1: explicit null-check (was `|| 120`, which
    // swallowed an operator-configured `0`). Server-side validation
    // already clamps `0` to the default, but keep the JS guard so a
    // future server-side change can't silently flip the dashboard
    // semantics.
    var staleThresholdSecs =
      data.stale_threshold_secs != null ? data.stale_threshold_secs : 120;
    var badThresholdSecs =
      data.bad_threshold_secs != null ? data.bad_threshold_secs : 86400;

    // Replace the grid contents atomically.
    while (els.grid.firstChild) {
      els.grid.removeChild(els.grid.firstChild);
    }
    if (!data.applications || data.applications.length === 0) {
      els.grid.appendChild(
        el("p", { class: "empty-application" }, ["No applications configured."])
      );
    } else {
      for (var i = 0; i < data.applications.length; i++) {
        els.grid.appendChild(
          renderApplication(data.applications[i], asOfMs, staleThresholdSecs, badThresholdSecs)
        );
      }
    }

    els.lastRefresh.textContent = dateFormatter.format(new Date());
  }

  function fetchDevices() {
    var controller = ABORT_SUPPORTED ? new AbortController() : null;
    if (ABORT_SUPPORTED && inflightToken !== null) {
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
            "Live metrics unavailable (HTTP " + resp.status + ")."
          );
          return null;
        }
        var ct = resp.headers.get("content-type") || "";
        if (ct.indexOf("application/json") === -1) {
          showError(
            "Live metrics unavailable (upstream returned non-JSON; check proxy / auth gateway configuration)."
          );
          return null;
        }
        clearError();
        return resp.json();
      })
      .then(function (data) {
        // Stale-render guard (mirrors dashboard.js M1).
        if (data && inflightToken === thisCallToken) {
          render(data);
        }
      })
      .catch(function (err) {
        if (err && err.name === "AbortError") {
          return;
        }
        if (inflightToken !== thisCallToken) {
          return;
        }
        showError(
          "Live metrics unavailable (network error). Check the gateway connection."
        );
      })
      .finally(function () {
        if (inflightToken === thisCallToken) {
          inflightToken = null;
        }
      });
  }

  els.refreshButton.addEventListener("click", fetchDevices);

  // Initial fetch + periodic refresh.
  fetchDevices();
  setInterval(fetchDevices, REFRESH_INTERVAL_MS);
})();
