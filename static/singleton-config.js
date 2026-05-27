// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz
//
// Story D-1: client-side controller for the singleton-config editor.
// Fetches the snapshot on load, renders 4 collapsible sections,
// per-section save with confirmation modal + supervisor-restart UX.

(function () {
  'use strict';

  const SECRET_PLACEHOLDER = '<set via config/secrets.toml>';
  const SECTIONS = ['global', 'chirpstack', 'opcua', 'web'];

  // Field-shape inference: pick the input control based on the value's
  // JSON type. Booleans → <select>; numbers → <input type=number>;
  // arrays → <textarea> one-per-line; strings → <input type=text>.
  function renderField(section, key, value) {
    const wrapper = document.createElement('div');
    wrapper.className = 'field';

    const label = document.createElement('label');
    label.htmlFor = `f-${section}-${key}`;
    label.textContent = key;
    wrapper.appendChild(label);

    const isSecret = value === SECRET_PLACEHOLDER;

    if (isSecret) {
      const span = document.createElement('span');
      span.className = 'field-secret';
      span.textContent = SECRET_PLACEHOLDER;
      const badge = document.createElement('span');
      badge.className = 'field-badge';
      badge.textContent = 'Managed via secrets.toml';
      span.appendChild(badge);
      wrapper.appendChild(span);
      wrapper.dataset.secret = 'true';
    } else if (typeof value === 'boolean') {
      const sel = document.createElement('select');
      sel.id = `f-${section}-${key}`;
      sel.dataset.type = 'bool';
      ['true', 'false'].forEach(v => {
        const opt = document.createElement('option');
        opt.value = v;
        opt.textContent = v;
        if (String(value) === v) opt.selected = true;
        sel.appendChild(opt);
      });
      wrapper.appendChild(sel);
    } else if (typeof value === 'number') {
      const inp = document.createElement('input');
      inp.type = 'number';
      inp.id = `f-${section}-${key}`;
      inp.value = value;
      inp.dataset.type = Number.isInteger(value) ? 'int' : 'float';
      wrapper.appendChild(inp);
    } else if (Array.isArray(value)) {
      const ta = document.createElement('textarea');
      ta.id = `f-${section}-${key}`;
      ta.rows = Math.max(value.length + 1, 3);
      ta.value = value.join('\n');
      ta.dataset.type = 'array';
      wrapper.appendChild(ta);
    } else if (value === null) {
      const inp = document.createElement('input');
      inp.type = 'text';
      inp.id = `f-${section}-${key}`;
      inp.value = '';
      inp.placeholder = '(null)';
      inp.dataset.type = 'string-or-null';
      wrapper.appendChild(inp);
    } else {
      // String fallback (also covers Option<String> with Some).
      const inp = document.createElement('input');
      inp.type = 'text';
      inp.id = `f-${section}-${key}`;
      inp.value = String(value);
      inp.dataset.type = 'string';
      wrapper.appendChild(inp);
    }

    wrapper.dataset.key = key;
    return wrapper;
  }

  function renderSection(section, data) {
    const details = document.createElement('details');
    details.className = 'section';
    details.open = true;
    const summary = document.createElement('summary');
    summary.textContent = `[${section}]`;
    details.appendChild(summary);

    const keys = Object.keys(data).sort();
    for (const k of keys) {
      details.appendChild(renderField(section, k, data[k]));
    }

    const actions = document.createElement('div');
    actions.className = 'actions';
    const saveBtn = document.createElement('button');
    saveBtn.textContent = `Save [${section}]`;
    saveBtn.dataset.section = section;
    saveBtn.addEventListener('click', () => onSaveClick(section, details));
    actions.appendChild(saveBtn);

    const errBox = document.createElement('div');
    errBox.className = 'error';
    errBox.id = `err-${section}`;
    actions.appendChild(errBox);

    details.appendChild(actions);
    return details;
  }

  // Read the form values back out into a JSON object suitable for PUT.
  function collectSection(section, sectionEl) {
    const out = {};
    const fields = sectionEl.querySelectorAll('.field');
    fields.forEach(f => {
      if (f.dataset.secret === 'true') return; // skip secrets
      const key = f.dataset.key;
      const ctrl = f.querySelector('input, textarea, select');
      if (!ctrl) return;
      const t = ctrl.dataset.type;
      const raw = ctrl.value;
      if (t === 'bool') {
        out[key] = raw === 'true';
      } else if (t === 'int') {
        out[key] = parseInt(raw, 10);
      } else if (t === 'float') {
        out[key] = parseFloat(raw);
      } else if (t === 'array') {
        out[key] = raw.split('\n').map(s => s.trim()).filter(s => s.length > 0);
      } else if (t === 'string-or-null') {
        out[key] = raw === '' ? null : raw;
      } else {
        out[key] = raw;
      }
    });
    return out;
  }

  let pendingSave = null;

  function onSaveClick(section, sectionEl) {
    pendingSave = { section, sectionEl };
    document.getElementById('confirm-section').textContent = `[${section}]`;
    document.getElementById('confirm-modal').classList.add('visible');
  }

  function closeModal() {
    document.getElementById('confirm-modal').classList.remove('visible');
    pendingSave = null;
  }

  async function performSave() {
    if (!pendingSave) return;
    const { section, sectionEl } = pendingSave;
    closeModal();
    const errEl = document.getElementById(`err-${section}`);
    errEl.textContent = '';

    const body = collectSection(section, sectionEl);
    try {
      const r = await fetch(`/api/config/singleton/${encodeURIComponent(section)}`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
        credentials: 'include',
      });
      if (r.status === 202) {
        document.getElementById('restart-notice').classList.add('visible');
        // Disable all save buttons after triggering restart.
        document.querySelectorAll('.actions button').forEach(b => b.disabled = true);
        return;
      }
      let msg;
      try {
        const resp = await r.json();
        msg = `HTTP ${r.status} — ${resp.reason || resp.error || 'unknown'}: ${resp.hint || ''}`;
      } catch (_) {
        msg = `HTTP ${r.status} — request failed`;
      }
      errEl.textContent = msg;
    } catch (e) {
      errEl.textContent = `Network error: ${e}`;
    }
  }

  async function loadSnapshot() {
    try {
      const r = await fetch('/api/config/singleton', { credentials: 'include' });
      if (!r.ok) {
        throw new Error(`HTTP ${r.status}`);
      }
      const data = await r.json();
      const sectionsEl = document.getElementById('sections');
      sectionsEl.innerHTML = '';
      for (const s of SECTIONS) {
        if (data[s]) {
          sectionsEl.appendChild(renderSection(s, data[s]));
        }
      }
    } catch (e) {
      const sectionsEl = document.getElementById('sections');
      sectionsEl.innerHTML = `<div class="error">Failed to load config: ${e}</div>`;
    }
  }

  document.addEventListener('DOMContentLoaded', () => {
    document.getElementById('confirm-cancel').addEventListener('click', closeModal);
    document.getElementById('confirm-ok').addEventListener('click', performSave);
    loadSnapshot();
  });
})();
