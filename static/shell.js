// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz
//
// Story F-1: unified web shell — shared navigation/header.
//
// Every operator page includes <script src="/shell.js"></script> and this
// component injects ONE consistent nav/header bar at the top of <body>,
// replacing the 7-link <nav> that each page used to hand-duplicate. The active
// link is derived at runtime from location.pathname, so adding or renaming a
// nav entry is a single-file change here (no per-page <strong> editing).
//
// Vanilla, no build step, no framework, no node_modules — the same
// self-contained-component pattern as static/apply-bar.js (Story F-0). Styling
// lives in static/dashboard.css (the ".app-shell" component section) so every
// page that already links dashboard.css picks it up.
//
// Design note: the shell only injects the shared CHROME (brand + nav). It does
// NOT relocate page content — each page keeps its own <main>/<header> content,
// DOM IDs, <meta viewport>, and <script> tags in the served HTML, so the
// server-side markup assertions in tests/web_dashboard.rs stay valid.

(function () {
  'use strict';

  // Single source of truth for the primary navigation.
  var NAV = [
    { href: '/index.html', label: 'Dashboard' },
    { href: '/applications.html', label: 'Applications' },
    { href: '/devices-config.html', label: 'Devices configuration' },
    { href: '/metrics.html', label: 'Live Metrics' },
    { href: '/commands.html', label: 'Commands' },
    { href: '/inventory-drift.html', label: 'Inventory drift' },
    { href: '/singleton-config.html', label: 'Singleton config' },
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

    var nav = document.createElement('nav');
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
