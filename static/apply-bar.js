// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz
//
// Story F-0: minimal "pending changes / Apply" affordance.
//
// Every config-write surface (the singleton-config editor and the
// application/device/metric/command CRUD editors) now STAGES its change to
// SQLite without applying it live. This shared component renders a banner
// that appears whenever there are unapplied changes (polled from
// `GET /api/status` -> `pending_changes`) and an "Apply changes" button that
// POSTs to `/api/config/apply`. Apply performs ONE in-process soft restart of
// the data-plane (the Docker container is never restarted); OPC UA clients
// briefly disconnect and reconnect once per applied batch.
//
// This is the deliberately MINIMAL version. The polished, shell-integrated
// affordance is Story F-1's job — keep this self-contained (it injects its
// own styles + DOM) so any config page can include it with a single
// <script src="/apply-bar.js"></script>.

(function () {
  'use strict';

  const POLL_MS = 4000;
  let applying = false;

  function injectStyles() {
    const css = `
      #apply-bar {
        position: fixed; left: 0; right: 0; bottom: 0; z-index: 2000;
        display: none; align-items: center; gap: 1em;
        padding: 0.75em 1.25em; box-sizing: border-box;
        background: #fff3cd; border-top: 2px solid #ffc107;
        font-family: system-ui, sans-serif; box-shadow: 0 -2px 8px rgba(0,0,0,0.15);
      }
      #apply-bar.visible { display: flex; }
      #apply-bar .apply-bar-msg { flex: 1; min-width: 0; }
      #apply-bar button {
        padding: 0.5em 1.1em; cursor: pointer; font-weight: bold;
        border: 1px solid #997404; border-radius: 4px; background: #ffca2c;
      }
      #apply-bar button:disabled { cursor: progress; opacity: 0.6; }
      #apply-bar.applying { background: #cfe2ff; border-top-color: #0d6efd; }
    `;
    const style = document.createElement('style');
    style.textContent = css;
    document.head.appendChild(style);
  }

  function buildBar() {
    const bar = document.createElement('div');
    bar.id = 'apply-bar';
    bar.setAttribute('role', 'status');

    const msg = document.createElement('span');
    msg.className = 'apply-bar-msg';
    msg.id = 'apply-bar-msg';
    msg.innerHTML =
      '<strong>Unapplied configuration changes.</strong> ' +
      'Your edits are staged but not yet live. Click <em>Apply changes</em> ' +
      'to soft-restart the gateway and put them into effect.';

    const btn = document.createElement('button');
    btn.id = 'apply-bar-btn';
    btn.type = 'button';
    btn.textContent = 'Apply changes';
    btn.addEventListener('click', onApplyClick);

    bar.appendChild(msg);
    bar.appendChild(btn);
    document.body.appendChild(bar);
  }

  function show(pending) {
    const bar = document.getElementById('apply-bar');
    if (!bar) return;
    if (applying) return; // don't fight the applying UI
    bar.classList.toggle('visible', !!pending);
  }

  async function poll() {
    if (applying) return;
    try {
      const r = await fetch('/api/status', { credentials: 'include' });
      if (!r.ok) return;
      const s = await r.json();
      show(!!s.pending_changes);
      // Surface a prior failed Apply even if this tab did not initiate it
      // (the failure leaves pending_changes true, so the bar is visible).
      if (s.pending_changes && s.apply_failed) {
        const msg = document.getElementById('apply-bar-msg');
        if (msg) {
          msg.innerHTML =
            '<strong>Last Apply failed.</strong> The staged configuration ' +
            'could not be applied; the gateway is still running the previous ' +
            'configuration. Check the gateway logs (<code>event=apply_failed</code>), ' +
            'fix the staged change, then click <em>Apply changes</em> again.';
        }
      }
    } catch (_) {
      // Network blip — leave the bar in its current state; next tick retries.
    }
  }

  async function onApplyClick() {
    const bar = document.getElementById('apply-bar');
    const btn = document.getElementById('apply-bar-btn');
    const msg = document.getElementById('apply-bar-msg');
    if (!bar || !btn || !msg) return;

    applying = true;
    btn.disabled = true;
    bar.classList.add('visible', 'applying');
    msg.innerHTML =
      '<strong>Applying…</strong> the gateway is performing an in-process ' +
      'soft restart. OPC UA clients will briefly disconnect and reconnect. ' +
      'The container is not restarted.';

    try {
      const r = await fetch('/api/config/apply', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        credentials: 'include',
      });
      if (r.status !== 202) {
        let detail = `HTTP ${r.status}`;
        try {
          const body = await r.json();
          detail += ` — ${body.reason || body.error || 'apply rejected'}`;
        } catch (_) { /* non-JSON body */ }
        msg.innerHTML = `<strong>Apply failed.</strong> ${detail}. Your changes remain staged.`;
        bar.classList.remove('applying');
        applying = false;
        btn.disabled = false;
        return;
      }
    } catch (e) {
      msg.innerHTML = `<strong>Apply request failed.</strong> ${e}. Your changes remain staged.`;
      bar.classList.remove('applying');
      applying = false;
      btn.disabled = false;
      return;
    }

    // 202 accepted: the data-plane is soft-restarting. The embedded web server
    // persists across the restart, so /api/status stays reachable; wait for
    // pending_changes to flip back to false, then clear the bar.
    await waitForApplied(msg, bar, btn);
  }

  async function waitForApplied(msg, bar, btn) {
    const deadline = Date.now() + 30000;
    while (Date.now() < deadline) {
      await sleep(1000);
      try {
        const r = await fetch('/api/status', { credentials: 'include' });
        if (r.ok) {
          const s = await r.json();
          // Failure: the supervisor could not apply the staged config and
          // kept the previous one running. pending_changes stays true, so
          // detect the explicit apply_failed flag and surface it instead of
          // hanging until the timeout (review D2).
          if (s.apply_failed) {
            applying = false;
            btn.disabled = false;
            bar.classList.remove('applying');
            msg.innerHTML =
              '<strong>Apply failed.</strong> The staged configuration could not ' +
              'be applied; the gateway is still running the previous configuration. ' +
              'Check the gateway logs (<code>event=apply_failed</code>), fix the ' +
              'staged change, then click <em>Apply changes</em> again.';
            return;
          }
          if (!s.pending_changes) {
            applying = false;
            btn.disabled = false;
            bar.classList.remove('applying');
            msg.innerHTML = '<strong>Changes applied.</strong> The gateway is running the new configuration.';
            setTimeout(() => bar.classList.remove('visible'), 2500);
            return;
          }
        }
      } catch (_) {
        // Web server briefly unreachable mid-restart is acceptable; keep waiting.
      }
    }
    // Timed out waiting for confirmation — fall back to normal polling.
    applying = false;
    btn.disabled = false;
    bar.classList.remove('applying');
    msg.innerHTML =
      '<strong>Apply requested.</strong> If the banner persists, reload the page ' +
      'to re-check the pending-changes status.';
  }

  function sleep(ms) {
    return new Promise(resolve => setTimeout(resolve, ms));
  }

  document.addEventListener('DOMContentLoaded', () => {
    injectStyles();
    buildBar();
    poll();
    setInterval(poll, POLL_MS);
  });
})();
