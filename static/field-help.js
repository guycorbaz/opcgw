// Story G-2 (#142): contextual field-help catalog + accessible affordance.
//
// ONE source of truth for the web UI's per-field help text. The strings
// are derived from docs/configuration.md (the canonical field reference) —
// keep the two in sync when a field's meaning, range, or units change.
//
// Vanilla, no build step. Exposes window.opcgwHelp:
//   - opcgwHelp.has(fieldKey)                  -> bool
//   - opcgwHelp.text(fieldKey)                 -> { text, docHref } | null
//   - opcgwHelp.affordance(fieldKey, inputEl)  -> Node | null
//        Returns an accessible help node (info-icon <button> toggle + a
//        role="note" region) and wires aria-describedby on inputEl. The
//        CALLER appends the returned node where it wants (dynamic forms:
//        config.js, singleton-config.js). Returns null for unknown keys.
//   - opcgwHelp.attachByData(root)             -> void
//        Scans root (default document) for [data-help] elements and
//        inserts an affordance after each. Used by STATIC pages
//        (setup.html). Runs automatically on DOMContentLoaded.
//
// Accessibility: the toggle is a real <button> (Enter/Space), carries
// aria-expanded, and the revealed text is a role="note" region linked to
// the input via aria-describedby; Escape closes an open popover. This is
// keyboard- and screen-reader-reachable, unlike a bare title= tooltip.

(function () {
  'use strict';

  // ---- Help catalog. Keyed {section/form}.{field}. -------------------
  // Text mirrors docs/configuration.md; keep terse + operator-oriented.
  var HELP = {
    // First-run setup wizard.
    'setup.server_address': { text: 'Full ChirpStack gRPC server URL including scheme and port, e.g. http://chirpstack:8080 or https://cs.example.com:8080. Must start with http:// or https://.' },
    'setup.tenant_id': { text: 'The ChirpStack tenant whose devices this gateway exposes (ChirpStack UI → Tenants). opcgw only sees devices under this tenant.' },
    'setup.api_token': { text: 'A ChirpStack API key with read access to the tenant’s applications and devices (ChirpStack UI → API Keys). Stored as a secret in config/secrets.toml — never in the database or logs.' },
    'setup.password': { text: 'Password your SCADA clients use to connect over OPC UA (and to the web UI; the user is [opcua].user_name). Must not be empty, no leading/trailing whitespace, no control characters, not a placeholder string, up to 256 characters. Stored as a secret.' },
    'setup.password_confirm': { text: 'Re-type the same OPC UA password to confirm it.' },

    // [global]
    'global.debug': { text: 'Enable verbose debug logging. Leave off in production for better performance.' },
    'global.prune_interval_minutes': { text: 'How often (minutes) opcgw prunes expired stored metric/command history.' },
    'global.history_retention_days': { text: 'How many days of metric/command history to keep before pruning.' },
    'global.command_delivery_poll_interval_secs': { text: 'How often (seconds, ≥1) opcgw checks ChirpStack for command-delivery confirmations.' },
    'global.command_delivery_timeout_secs': { text: 'A command left in the "sent" state longer than this (seconds, ≥1) is marked failed.' },
    'global.command_timeout_check_interval_secs': { text: 'How often (seconds) opcgw sweeps for timed-out commands.' },

    // [chirpstack]
    'chirpstack.server_address': { text: 'ChirpStack gRPC server address as scheme://host:port. Must start with http:// or https://.' },
    'chirpstack.api_token': { text: 'ChirpStack API token. Managed as a secret in config/secrets.toml; edit it there, not here.' },
    'chirpstack.tenant_id': { text: 'ChirpStack Tenant ID. opcgw only sees devices under this tenant.' },
    'chirpstack.polling_frequency': { text: 'Seconds between metric polls of ChirpStack. Must be > 0; 5–300 is typical.' },
    'chirpstack.retry': { text: 'Maximum connection-retry attempts on failure. Must be > 0; 3–10 typical.' },
    'chirpstack.delay': { text: 'Milliseconds to wait between retry attempts. Must be > 0; 100–1000 typical.' },
    'chirpstack.stream_all_devices': { text: 'When on, opcgw streams uplink events for ALL devices, not just command-class ones. The device set is fixed at startup — changing this needs a restart.' },
    'chirpstack.list_page_size': { text: 'Page size for ChirpStack list calls (applications/devices). Tuning knob; default 100.' },
    'chirpstack.inventory_cache_ttl_seconds': { text: 'How long (seconds) the web UI caches ChirpStack inventory (applications/devices/measurements) before refetching.' },
    'chirpstack.inventory_uplink_max_wait_seconds': { text: 'Maximum seconds the metric picker waits when reading recent uplinks for a device.' },

    // [opcua]
    'opcua.application_name': { text: 'OPC UA application name advertised to clients in the server endpoint.' },
    'opcua.application_uri': { text: 'OPC UA application URI (must match the URI in the server certificate, if one is used).' },
    'opcua.product_uri': { text: 'OPC UA product URI advertised in the server description.' },
    'opcua.diagnostics_enabled': { text: 'Expose OPC UA server diagnostic nodes. Off by default.' },
    'opcua.hello_timeout': { text: 'Seconds the server waits for a client Hello before closing a new connection.' },
    'opcua.host_ip_address': { text: 'IP address the OPC UA server binds to. 0.0.0.0 = all interfaces; set a specific IP to restrict access.' },
    'opcua.host_port': { text: 'TCP port the OPC UA server listens on (1–65535, not 0). Default 4840.' },
    'opcua.create_sample_keypair': { text: 'Auto-generate a self-signed certificate if none exists. OK for testing; use proper certs in production.' },
    'opcua.certificate_path': { text: 'Path (within pki_dir) to the server certificate (DER).' },
    'opcua.private_key_path': { text: 'Path (within pki_dir) to the server private key (PEM).' },
    'opcua.trust_client_cert': { text: 'Automatically trust client certificates instead of requiring manual approval. Use with care.' },
    'opcua.check_cert_time': { text: 'Reject client certificates outside their validity period.' },
    'opcua.pki_dir': { text: 'Directory holding OPC UA PKI files (own / trusted / rejected certs). Use restricted permissions.' },
    'opcua.user_name': { text: 'Username OPC UA clients (and the web UI) authenticate with.' },
    'opcua.user_password': { text: 'OPC UA / web-UI password. Managed as a secret in config/secrets.toml; edit it there, not here.' },
    'opcua.stale_threshold_seconds': { text: 'Age (seconds) past which a metric’s OPC UA status degrades to Uncertain; older than 24 h returns Bad. Range (0, 86400]. Can be overridden per device.' },
    'opcua.max_connections': { text: 'Maximum concurrent OPC UA client sessions (optional cap).' },
    'opcua.max_subscriptions_per_session': { text: 'Maximum OPC UA subscriptions allowed per client session (optional cap).' },
    'opcua.max_monitored_items_per_sub': { text: 'Maximum monitored items per OPC UA subscription (optional cap).' },
    'opcua.max_message_size': { text: 'Maximum OPC UA message size in bytes (optional cap).' },
    'opcua.max_chunk_count': { text: 'Maximum number of chunks per OPC UA message (optional cap).' },
    'opcua.max_history_data_results_per_node': { text: 'Maximum history values returned per node in one OPC UA history read (optional cap).' },

    // [web]
    'web.port': { text: 'TCP port the web configuration UI listens on. The server is HTTP-only — put a reverse proxy in front for TLS.' },
    'web.bind_address': { text: 'IP address the web UI binds to. 0.0.0.0 = all interfaces; set a specific IP to restrict access.' },
    'web.auth_realm': { text: 'HTTP Basic auth realm shown in the browser login prompt (max 64 chars).' },
    'web.enabled': { text: 'Enable the web configuration UI. When off, opcgw runs headless (config via files only).' },
    'web.allowed_origins': { text: 'CSRF allow-list for state-changing web requests — one scheme://host[:port] per line. Defaults to the bind address.' },

    // Device / metric / command forms (config.js).
    'device.stale_threshold_seconds': { text: 'Optional per-device override of the global OPC UA stale threshold. Whole seconds in (0, 86400]. Leave empty to use the global [opcua].stale_threshold_seconds (default 120 s). Useful for slow sensors that would otherwise read Uncertain between uplinks.' },
    'metric.metric_name': { text: 'Display name of the variable in the OPC UA address space (what SCADA clients see).' },
    'metric.chirpstack_metric_name': { text: 'The exact field name from the ChirpStack decoded uplink (or device-profile measurement) to read. Must match ChirpStack exactly.' },
    'metric.metric_type': { text: 'OPC UA data type the value is exposed as: Float, Int, Bool, or String. The picker can infer this from the device profile or recent uplinks.' },
    'metric.metric_unit': { text: 'Optional engineering unit shown with the value (e.g. °C, %, kW).' },
    'command.command_id': { text: 'Unique numeric identifier for this command on the device.' },
    'command.command_name': { text: 'Display name of the writable command node in the OPC UA address space.' },
    'command.command_port': { text: 'LoRaWAN f_port (1–223) the downlink is sent on.' },
    'command.command_confirmed': { text: 'Require ChirpStack to return a delivery confirmation (confirmed downlink) for this command.' },
    'command.command_class': { text: 'Optional device-class binding. Empty = raw payload bytes on the f_port (legacy, model-specific). "valve" maps canonical OPC UA 1 → open / 0 → close via the ChirpStack device-profile codec, keeping opcgw model-agnostic.' }
  };

  var _seq = 0;
  var _warned = {};

  function sanitiseId(fieldKey) {
    return String(fieldKey).replace(/[^a-zA-Z0-9_-]/g, '-');
  }

  // Build the affordance (info-icon toggle + hidden help region) for a
  // field and wire aria-describedby on inputEl. Returns the wrapper node,
  // or null if the field key is unknown.
  function affordance(fieldKey, inputEl) {
    var entry = HELP[fieldKey];
    if (!entry) {
      if (!_warned[fieldKey]) {
        _warned[fieldKey] = true;
        // eslint-disable-next-line no-console
        console.warn('opcgwHelp: no help text for field "' + fieldKey + '"');
      }
      return null;
    }

    var helpId = 'help-' + sanitiseId(fieldKey) + '-' + (_seq++);

    var wrap = document.createElement('span');
    wrap.className = 'field-help';

    var btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'field-help-toggle';
    btn.setAttribute('aria-label', 'Help: ' + fieldKey);
    btn.setAttribute('aria-expanded', 'false');
    btn.setAttribute('aria-controls', helpId);
    btn.textContent = 'ⓘ'; // ⓘ

    var note = document.createElement('span');
    note.className = 'field-help-text';
    note.id = helpId;
    note.setAttribute('role', 'note');
    note.hidden = true;
    note.appendChild(document.createTextNode(entry.text));
    if (entry.docHref) {
      note.appendChild(document.createTextNode(' '));
      var a = document.createElement('a');
      a.href = entry.docHref;
      a.target = '_blank';
      a.rel = 'noopener';
      a.textContent = 'Learn more';
      note.appendChild(a);
    }

    function setOpen(open) {
      note.hidden = !open;
      btn.setAttribute('aria-expanded', open ? 'true' : 'false');
    }
    btn.addEventListener('click', function () {
      setOpen(note.hidden);
    });
    note.addEventListener('keydown', function (ev) {
      if (ev.key === 'Escape') { setOpen(false); btn.focus(); }
    });
    btn.addEventListener('keydown', function (ev) {
      if (ev.key === 'Escape') { setOpen(false); }
    });

    // Link the help text to the input for assistive tech, preserving any
    // existing aria-describedby (e.g. a wizard hint).
    if (inputEl && inputEl.setAttribute) {
      var existing = inputEl.getAttribute('aria-describedby');
      inputEl.setAttribute('aria-describedby', existing ? existing + ' ' + helpId : helpId);
    }

    wrap.appendChild(btn);
    wrap.appendChild(note);
    return wrap;
  }

  // Static-page entry point: for each [data-help] element, build an
  // affordance keyed by its data-help value and insert it right after the
  // element (so it sits next to the field control).
  function attachByData(root) {
    var scope = root || document;
    var nodes = scope.querySelectorAll('[data-help]');
    Array.prototype.forEach.call(nodes, function (elm) {
      if (elm.dataset.opcgwHelpAttached === '1') return;
      var node = affordance(elm.dataset.help, elm);
      if (!node) return;
      elm.dataset.opcgwHelpAttached = '1';
      if (elm.parentNode) {
        elm.parentNode.insertBefore(node, elm.nextSibling);
      }
    });
  }

  window.opcgwHelp = {
    has: function (fieldKey) { return Object.prototype.hasOwnProperty.call(HELP, fieldKey); },
    text: function (fieldKey) { return HELP[fieldKey] || null; },
    affordance: affordance,
    attachByData: attachByData,
    // Exposed for the coverage self-check / tests.
    _keys: function () { return Object.keys(HELP); }
  };

  document.addEventListener('DOMContentLoaded', function () { attachByData(document); });
})();
