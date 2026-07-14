// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz
//
// Story F-1: unified web shell — shared navigation/header.
// Story I-2 (#147): restyled as ChirpStack-adjacent chrome — a fixed left
// sider on wide viewports and a top app-bar with an accessible hamburger
// drawer on narrow ones. The layout split lives entirely in
// static/dashboard.css (".app-shell" section); this file only builds the DOM.
//
// Every operator page includes <script src="/shell.js"></script> and this
// component injects ONE consistent chrome element at the top of <body>,
// replacing the 7-link <nav> that each page used to hand-duplicate. The active
// link is derived at runtime from location.pathname, so adding or renaming a
// nav entry is a single-file change here (no per-page <strong> editing).
//
// Vanilla, no build step, no framework, no node_modules — the same
// self-contained-component pattern as static/apply-bar.js (Story F-0).
//
// Design note: the shell only injects the shared CHROME (brand + nav +
// drawer toggle). It does NOT relocate page content — each page keeps its own
// <main>/<header> content, DOM IDs, <meta viewport>, and <script> tags in the
// served HTML, so the server-side markup assertions in tests/web_dashboard.rs
// stay valid. The "has-shell" class added to <body> is what dashboard.css
// gates the sider offset on, so pages WITHOUT the shell (the first-run wizard
// links dashboard.css too) keep their normal full-width layout.

(function () {
  'use strict';

  // Single source of truth for the primary navigation.
  // Story G-0: the three flat config links (Applications / Devices configuration
  // / Commands) collapsed into ONE "Configuration" drill-down (Application →
  // Device → Metrics/Commands) at /config.html.
  var NAV = [
    { href: '/index.html', label: 'Dashboard' },
    { href: '/config.html', label: 'Configuration' },
    { href: '/metrics.html', label: 'Live Metrics' },
    { href: '/inventory-drift.html', label: 'Inventory drift' },
    { href: '/singleton-config.html', label: 'Admin' },
  ];

  // Normalise the current path so "/" and "" resolve to the dashboard.
  function currentPath() {
    var p = window.location.pathname;
    if (p === '/' || p === '') return '/index.html';
    return p;
  }

  function buildShell() {
    // Idempotent: never inject a second bar (e.g. if double-included).
    if (document.querySelector('.app-shell')) return;

    var header = document.createElement('header');
    header.className = 'app-shell';

    var brand = document.createElement('a');
    brand.className = 'app-shell__brand';
    brand.href = '/index.html';
    brand.textContent = 'opcgw';
    header.appendChild(brand);

    // Story I-2: hamburger toggle for the narrow-viewport drawer. Hidden by
    // CSS on wide viewports (where the nav is the always-visible sider).
    var toggle = document.createElement('button');
    toggle.type = 'button';
    toggle.className = 'app-shell__toggle';
    toggle.setAttribute('aria-expanded', 'false');
    toggle.setAttribute('aria-controls', 'app-shell-nav');
    toggle.setAttribute('aria-label', 'Toggle navigation');
    toggle.textContent = '☰'; // ☰ (aria-label supplies the accessible name)
    header.appendChild(toggle);

    var nav = document.createElement('nav');
    nav.id = 'app-shell-nav';
    nav.className = 'app-shell__nav';
    nav.setAttribute('aria-label', 'Primary navigation');

    var here = currentPath();
    NAV.forEach(function (item) {
      var a = document.createElement('a');
      a.href = item.href;
      a.textContent = item.label;
      if (item.href === here) {
        a.className = 'is-active';
        a.setAttribute('aria-current', 'page');
      }
      nav.appendChild(a);
    });
    header.appendChild(nav);

    toggle.addEventListener('click', function () {
      var open = header.classList.toggle('app-shell--open');
      toggle.setAttribute('aria-expanded', open ? 'true' : 'false');
    });

    // The sider/app-bar layout in dashboard.css is gated on this class so the
    // shell-less first-run wizard (which links the same stylesheet) is
    // unaffected.
    document.body.classList.add('has-shell');

    // Insert as the very first element of <body> so the nav is always on top,
    // regardless of where this script tag sits in the page.
    document.body.insertBefore(header, document.body.firstChild);
  }

  // The script may be included at end-of-body (body already present) or, in
  // principle, earlier — handle both.
  if (document.body) {
    buildShell();
  } else {
    document.addEventListener('DOMContentLoaded', buildShell);
  }
})();
