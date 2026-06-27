// opcgw dashboard — Story 9-2 (FR38) + Story F-3 (landing redesign).
//
// Polls /api/status AND /api/devices on load + every 10 s. No SPA
// framework, no build step. Browser caches Basic-auth credentials per
// realm; 401 is treated as "credentials revoked" — operator gets an
// inline banner and can reload to re-prompt.
//
// Story F-3 adds: an at-a-glance overall health verdict (derived
// CLIENT-SIDE from /api/status + /api/devices — no gateway-side
// aggregation, #130), a Poller status tile (stall detection from
// last_poll age vs poll_interval_secs), and a per-device freshness
// summary (fresh/stale/bad/never, same band model as metrics.html).
//
// Story 9-2 fetch hardening preserved (now factored into makePoller so
// BOTH endpoints get it): in-flight guard (E2), per-call AbortController
// with feature-detect (E5/M2), stale-render guard (M1), Content-Type
// sniff (E8), 401 handling, generic network-error banner (B9), single
// timestamp parse pass (E13). /api/devices failing degrades gracefully:
// the /api/status-only verdict still renders.

(function () {
  "use strict";

  var REFRESH_INTERVAL_MS = 10000;

  var els = {
    healthSummary: document.getElementById("health-summary"),
    healthHeadline: document.getElementById("health-headline"),
    healthDetail: document.getElementById("health-detail"),
    chirpstack: document.getElementById("chirpstack-status"),
    pollerStatus: document.getElementById("poller-status"),
    pollerHint: document.getElementById("poller-hint"),
    pollInterval: document.getElementById("poll-interval"),
    lastPollRel: document.getElementById("last-poll-relative"),
    lastPollTime: document.getElementById("last-poll-time"),
    errorCount: document.getElementById("error-count"),
    appCount: document.getElementById("application-count"),
    devCount: document.getElementById("device-count"),
    uptime: document.getElementById("uptime"),
    freshFresh: document.getElementById("freshness-fresh"),
    freshStale: document.getElementById("freshness-stale"),
    freshBad: document.getElementById("freshness-bad"),
    freshNever: document.getElementById("freshness-never"),
    freshHint: document.getElementById("freshness-hint"),
    lastRefresh: document.getElementById("last-refresh"),
    errorBanner: document.getElementById("error-banner"),
    refreshButton: document.getElementById("refresh-now"),
  };

  var numberFormatter = new Intl.NumberFormat();
  var dateFormatter = new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "medium",
  });

  // Latest payloads — the health verdict depends on BOTH, so each fetch
  // updates its slice and re-derives the verdict. `lastFreshness` is
  // `undefined` until the first /api/devices result, and the sentinel
  // string "unavailable" if /api/devices failed (verdict degrades).
  var lastStatus = null;
  var lastFreshness = undefined;

  // ---- pure helpers (side-effect-free; exercised by node --check) -------

  // Parse an ISO string once and return both representations the
  // dashboard needs (Story 9-2 E13: avoid the "—" vs raw-ISO mismatch).
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
    if (deltaSecs < 60) return deltaSecs + " s ago";
    if (deltaSecs < 3600) return Math.floor(deltaSecs / 60) + " min ago";
    if (deltaSecs < 86400) return Math.floor(deltaSecs / 3600) + " h ago";
    return Math.floor(deltaSecs / 86400) + " d ago";
  }

  function formatDuration(secs) {
    if (typeof secs !== "number" || secs < 0) return "—";
    var d = Math.floor(secs / 86400);
    var h = Math.floor((secs % 86400) / 3600);
    var m = Math.floor((secs % 3600) / 60);
    var s = secs % 60;
    if (d > 0) return d + "d " + h + "h";
    if (h > 0) return h + "h " + m + "m";
    if (m > 0) return m + "m " + s + "s";
    return s + "s";
  }

  function formatInterval(secs) {
    if (typeof secs !== "number" || secs <= 0) return "—";
    if (secs < 60) return secs + " s";
    if (secs % 60 === 0) return secs / 60 + " min";
    return Math.floor(secs / 60) + " min " + (secs % 60) + " s";
  }

  // Poller "stalled" iff a poll HAS happened but the most recent one is
  // older than ~3× the poll interval (floor 60 s). A null last_poll_time
  // is "never polled" (handled separately), not "stalled".
  function pollerStalled(status) {
    var parsed = parseTimestamp(status.last_poll_time);
    if (!parsed.ok) return false;
    var ageSecs = Math.max(0, Math.floor((Date.now() - parsed.ms) / 1000));
    var interval =
      typeof status.poll_interval_secs === "number" &&
      status.poll_interval_secs > 0
        ? status.poll_interval_secs
        : 60;
    return ageSecs > Math.max(60, interval * 3);
  }

  // Single-metric band — byte-identical to metrics.html `statusFor`
  // (good / uncertain / bad / missing) so the dashboard freshness counts can
  // never disagree with what metrics.html colours for the same device. Code
  // review iter-1: `statusFor` treats ONLY null/undefined as missing (NOT an
  // empty string), so a "" value with a fresh timestamp bands by age — match
  // that exactly here (the value cell still renders "—" on metrics.html, but
  // the band/colour is age-driven on both pages).
  function metricBand(metric, asOfMs, staleSecs, badSecs) {
    if (metric.value === null || metric.value === undefined) {
      return "missing";
    }
    var parsed = parseTimestamp(metric.timestamp);
    if (!parsed.ok) return "missing";
    var ageSecs = Math.max(0, Math.floor((asOfMs - parsed.ms) / 1000));
    if (ageSecs >= badSecs) return "bad";
    if (ageSecs >= staleSecs) return "uncertain";
    return "good";
  }

  // Device band = worst of its metrics. Precedence: bad > stale > fresh;
  // a device with no value at all (or no metrics) is "never".
  function deviceBand(device, asOfMs, staleSecs, badSecs) {
    var hasBad = false,
      hasUncertain = false,
      hasGood = false,
      count = 0;
    var metrics = device.metrics || [];
    for (var i = 0; i < metrics.length; i++) {
      count++;
      var b = metricBand(metrics[i], asOfMs, staleSecs, badSecs);
      if (b === "bad") hasBad = true;
      else if (b === "uncertain") hasUncertain = true;
      else if (b === "good") hasGood = true;
    }
    if (count === 0) return "never";
    if (hasBad) return "bad";
    if (hasUncertain) return "stale";
    if (hasGood) return "fresh";
    return "never";
  }

  function summariseFreshness(devicesResponse) {
    var asOf = parseTimestamp(devicesResponse.as_of);
    var asOfMs = asOf.ok ? asOf.ms : Date.now();
    // Defensive thresholds (code review iter-1 M3): if the server omits or
    // sends a non-positive threshold, fall back to the documented defaults
    // (same guard metrics.js carries) so a single bad field can't flip every
    // device to "bad"/"fresh" and corrupt the top-level health verdict.
    var staleSecs =
      typeof devicesResponse.stale_threshold_secs === "number" &&
      devicesResponse.stale_threshold_secs > 0
        ? devicesResponse.stale_threshold_secs
        : 120;
    var badSecs =
      typeof devicesResponse.bad_threshold_secs === "number" &&
      devicesResponse.bad_threshold_secs > 0
        ? devicesResponse.bad_threshold_secs
        : 86400;
    var counts = { fresh: 0, stale: 0, bad: 0, never: 0, total: 0 };
    var apps = devicesResponse.applications || [];
    for (var a = 0; a < apps.length; a++) {
      var devs = apps[a].devices || [];
      for (var d = 0; d < devs.length; d++) {
        // Story G-3 (#132): honour a per-device stale threshold when set
        // (and valid); otherwise fall back to the global default. Same band
        // model — just a per-device staleSecs.
        var devStale =
          typeof devs[d].stale_threshold_seconds === "number" &&
          devs[d].stale_threshold_seconds > 0
            ? devs[d].stale_threshold_seconds
            : staleSecs;
        var band = deviceBand(devs[d], asOfMs, devStale, badSecs);
        counts[band] += 1;
        counts.total += 1;
      }
    }
    return counts;
  }

  // Overall verdict (AC#1 precedence). `freshness` may be undefined
  // (not yet loaded) or "unavailable" (fetch failed) — both mean the
  // device-band branches are skipped and the verdict falls back to the
  // /api/status signals.
  function computeVerdict(status, freshness) {
    var fresh = freshness && freshness !== "unavailable" ? freshness : null;
    if (status.chirpstack_available === false) {
      return {
        level: "error",
        headline: "ChirpStack unreachable",
        detail:
          "The gateway cannot reach ChirpStack — no new device data is arriving.",
      };
    }
    if (pollerStalled(status)) {
      return {
        level: "error",
        headline: "Poller stalled",
        detail:
          "No successful poll recently — the polling task may be stuck. Check the gateway logs.",
      };
    }
    if (status.apply_failed === true) {
      return {
        level: "error",
        headline: "Apply failed",
        detail:
          "The last configuration apply failed and was rolled back. Review the configuration and apply again.",
      };
    }
    if (fresh && fresh.bad > 0) {
      return {
        level: "error",
        headline: fresh.bad + " device(s) with no recent data",
        detail:
          "Some devices have not reported within the hard cutoff (shown as “bad” below).",
      };
    }
    if (fresh && fresh.stale > 0) {
      return {
        level: "warn",
        headline: fresh.stale + " device(s) going stale",
        detail:
          "Some devices have not reported within the stale threshold.",
      };
    }
    if (status.pending_changes === true) {
      return {
        level: "warn",
        headline: "Configuration changes pending",
        detail:
          "Edits are staged but not applied. Click Apply on the configuration page to activate them.",
      };
    }
    if ((status.application_count || 0) === 0) {
      return {
        level: "warn",
        headline: "No applications configured",
        detail: "Add an application to start polling devices.",
      };
    }
    if (status.chirpstack_available !== true) {
      return {
        level: "warn",
        headline: "ChirpStack status unknown",
        detail: "Waiting for the first poll outcome.",
      };
    }
    var lastPoll = parseTimestamp(status.last_poll_time);
    if (!lastPoll.ok && lastPoll.reason === "never") {
      return {
        level: "warn",
        headline: "Starting up",
        detail: "Waiting for the first poll to complete.",
      };
    }
    return {
      level: "ok",
      headline: "All systems operational",
      detail: "ChirpStack reachable and the poller is active.",
    };
  }

  // ---- DOM rendering ----------------------------------------------------

  // status-badge component (F-1): swap the is-ok/is-warn/is-error
  // modifier. `kind` ∈ {"ok","warn","error",null}; null = neutral.
  function setBadge(el, label, kind) {
    if (!el) return;
    el.textContent = label;
    el.classList.remove("is-ok", "is-warn", "is-error");
    if (kind) el.classList.add("is-" + kind);
  }

  // Per-poller error slots (code review iter-1 M1/M2): the status + devices
  // pollers must NOT clobber each other's banner. Each owns its key; the
  // banner shows the union of active errors and hides only when both clear.
  var bannerErrors = { status: null, devices: null };

  function renderBanner() {
    var msgs = [];
    if (bannerErrors.status) msgs.push(bannerErrors.status);
    if (bannerErrors.devices) msgs.push(bannerErrors.devices);
    if (msgs.length === 0) {
      els.errorBanner.textContent = "";
      els.errorBanner.classList.add("hidden");
    } else {
      els.errorBanner.textContent = msgs.join("  •  ");
      els.errorBanner.classList.remove("hidden");
    }
  }

  function setBannerError(key, message) {
    bannerErrors[key] = message;
    renderBanner();
  }

  function clearBannerError(key) {
    bannerErrors[key] = null;
    renderBanner();
  }

  function renderVerdict() {
    if (!lastStatus || !els.healthSummary) return;
    var v = computeVerdict(lastStatus, lastFreshness);
    els.healthSummary.classList.remove("is-ok", "is-warn", "is-error");
    els.healthSummary.classList.add("is-" + v.level);
    if (els.healthHeadline) els.healthHeadline.textContent = v.headline;
    if (els.healthDetail) els.healthDetail.textContent = v.detail;
  }

  function renderStatus(data) {
    // Story 9-2 E7: explicit `=== false`; anything not strictly
    // true/false renders "Unknown" (missing field ≠ ChirpStack down).
    if (data.chirpstack_available === true) {
      setBadge(els.chirpstack, "Available", "ok");
    } else if (data.chirpstack_available === false) {
      setBadge(els.chirpstack, "Unavailable", "error");
    } else {
      setBadge(els.chirpstack, "Unknown", null);
    }

    // Poller status.
    var parsed = parseTimestamp(data.last_poll_time);
    if (!parsed.ok && parsed.reason === "never") {
      setBadge(els.pollerStatus, "Starting", "warn");
    } else if (pollerStalled(data)) {
      setBadge(els.pollerStatus, "Stalled", "error");
    } else {
      setBadge(els.pollerStatus, "Active", "ok");
    }
    if (els.pollInterval) {
      els.pollInterval.textContent = formatInterval(data.poll_interval_secs);
    }

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
    var appCount = data.application_count || 0;
    els.appCount.textContent = numberFormatter.format(appCount);
    els.devCount.textContent = numberFormatter.format(data.device_count || 0);
    var hintEl = document.getElementById("application-hint");
    if (hintEl) {
      if (appCount === 0) {
        hintEl.innerHTML =
          'No applications configured yet. ' +
          '<a href="/applications.html">Add one</a> to start polling.';
      } else {
        hintEl.textContent = "Configured.";
      }
    }
    els.uptime.textContent = formatDuration(data.uptime_secs);
    els.lastRefresh.textContent = dateFormatter.format(new Date());
  }

  function renderFreshness(counts) {
    if (counts === "unavailable") {
      if (els.freshFresh) els.freshFresh.textContent = "—";
      if (els.freshStale) els.freshStale.textContent = "—";
      if (els.freshBad) els.freshBad.textContent = "—";
      if (els.freshNever) els.freshNever.textContent = "—";
      if (els.freshHint) {
        els.freshHint.textContent =
          "Device freshness is temporarily unavailable.";
      }
      return;
    }
    if (els.freshFresh) els.freshFresh.textContent = numberFormatter.format(counts.fresh);
    if (els.freshStale) els.freshStale.textContent = numberFormatter.format(counts.stale);
    if (els.freshBad) els.freshBad.textContent = numberFormatter.format(counts.bad);
    if (els.freshNever) els.freshNever.textContent = numberFormatter.format(counts.never);
  }

  // ---- generic hardened poller (Story 9-2 E2/E5/E8/M1/M2/B9) ------------

  var ABORT_SUPPORTED = typeof AbortController !== "undefined";

  function makePoller(key, label, url, onData, onUnavailable) {
    var inflightToken = null;
    return function poll() {
      var controller = ABORT_SUPPORTED ? new AbortController() : null;
      if (ABORT_SUPPORTED && inflightToken !== null) {
        inflightToken.abort();
      }
      var thisCallToken = controller !== null ? controller : {};
      inflightToken = thisCallToken;

      var fetchOpts = { cache: "no-store", credentials: "same-origin" };
      if (controller !== null) fetchOpts.signal = controller.signal;

      fetch(url, fetchOpts)
        .then(function (resp) {
          if (resp.status === 401) {
            setBannerError(
              key,
              label +
                " unavailable — session expired or credentials no longer accepted. Reload the page."
            );
            if (onUnavailable) onUnavailable();
            return null;
          }
          if (!resp.ok) {
            setBannerError(
              key,
              label + " unavailable (HTTP " + resp.status + ")."
            );
            if (onUnavailable) onUnavailable();
            return null;
          }
          var ct = resp.headers.get("content-type") || "";
          if (ct.indexOf("application/json") === -1) {
            setBannerError(
              key,
              label +
                " unavailable (upstream returned non-JSON; check proxy / auth gateway configuration)."
            );
            if (onUnavailable) onUnavailable();
            return null;
          }
          clearBannerError(key);
          return resp.json();
        })
        .then(function (data) {
          // M1 stale-render guard: drop data whose call was superseded.
          if (data && inflightToken === thisCallToken) {
            onData(data);
          }
        })
        .catch(function (err) {
          if (err && err.name === "AbortError") return;
          if (inflightToken !== thisCallToken) return;
          setBannerError(
            key,
            label + " unavailable (network error). Check the gateway connection."
          );
          if (onUnavailable) onUnavailable();
        })
        .finally(function () {
          if (inflightToken === thisCallToken) inflightToken = null;
        });
    };
  }

  var pollStatus = makePoller(
    "status",
    "Gateway status",
    "/api/status",
    function (data) {
      lastStatus = data;
      renderStatus(data);
      renderVerdict();
    },
    null // keep the last-good tiles on a status failure
  );

  // /api/devices feeds the freshness panel + the device-band verdict
  // branches. If it fails, its own banner slot surfaces it AND freshness is
  // marked "unavailable" so the verdict degrades to the /api/status-only
  // signals (does NOT blank the page or clobber the status banner).
  var pollDevices = makePoller(
    "devices",
    "Device data",
    "/api/devices",
    function (data) {
      lastFreshness = summariseFreshness(data);
      renderFreshness(lastFreshness);
      renderVerdict();
    },
    function () {
      lastFreshness = "unavailable";
      renderFreshness(lastFreshness);
      renderVerdict();
    }
  );

  function refreshAll() {
    pollStatus();
    pollDevices();
  }

  els.refreshButton.addEventListener("click", refreshAll);

  refreshAll();
  setInterval(refreshAll, REFRESH_INTERVAL_MS);

  // #128: one-time version fetch for the dashboard subtitle. Cosmetic —
  // failures are ignored so a hiccup never blocks the dashboard.
  fetch("/api/health", { credentials: "same-origin" })
    .then(function (r) { return r.ok ? r.json() : null; })
    .then(function (j) {
      if (j && j.version) {
        var elv = document.getElementById("app-version");
        if (elv) { elv.textContent = " · v" + j.version; }
      }
    })
    .catch(function () { /* version display is cosmetic; ignore */ });
})();
