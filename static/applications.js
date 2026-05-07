// Story 9-4: vanilla JS controller for /applications.html.
// No SPA framework; no build step; no npm install.
//
// All mutating fetches set Content-Type: application/json and
// rely on the browser to attach Origin (which the gateway's CSRF
// middleware checks against [web].allowed_origins).

(function () {
  const tbody = document.getElementById("applications-tbody");
  const listError = document.getElementById("list-error");
  const createForm = document.getElementById("create-form");
  const createError = document.getElementById("create-error");

  function showError(el, msg) {
    el.textContent = msg;
    el.hidden = false;
  }
  function clearError(el) {
    el.textContent = "";
    el.hidden = true;
  }

  async function fetchApplications() {
    clearError(listError);
    try {
      const r = await fetch("/api/applications", { credentials: "include" });
      if (!r.ok) {
        showError(listError, "Failed to load applications: HTTP " + r.status);
        return;
      }
      const data = await r.json();
      renderRows(data.applications || []);
    } catch (e) {
      showError(listError, "Failed to load applications: " + e);
    }
  }

  function renderRows(apps) {
    tbody.innerHTML = "";
    apps.forEach((app) => {
      const tr = document.createElement("tr");
      tr.innerHTML =
        "<td class=\"app-id\">" + escapeHtml(app.application_id) + "</td>" +
        "<td class=\"app-name\">" + escapeHtml(app.application_name) + "</td>" +
        "<td class=\"app-dev-count\">" + app.device_count + "</td>" +
        "<td class=\"actions\">" +
          "<button class=\"btn-edit\" data-id=\"" + escapeHtml(app.application_id) + "\" data-name=\"" + escapeHtml(app.application_name) + "\">Edit</button>" +
          "<button class=\"btn-delete\" data-id=\"" + escapeHtml(app.application_id) + "\">Delete</button>" +
        "</td>";
      tbody.appendChild(tr);
    });

    tbody.querySelectorAll(".btn-edit").forEach((btn) => {
      btn.addEventListener("click", () => onEdit(btn.dataset.id, btn.dataset.name));
    });
    tbody.querySelectorAll(".btn-delete").forEach((btn) => {
      btn.addEventListener("click", () => onDelete(btn.dataset.id));
    });
  }

  function escapeHtml(s) {
    // Story 9-4 review iter-1 P25: also escape the backtick.
    // Current HTML uses double-quoted attributes so a missing escape
    // is safe today, but a future refactor to template literals or
    // unquoted attributes would yield XSS without this.
    return String(s)
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;")
      .replace(/'/g, "&#39;")
      .replace(/`/g, "&#96;");
  }

  async function onEdit(applicationId, currentName) {
    const newName = window.prompt(
      "New application name for " + applicationId + ":",
      currentName
    );
    if (newName === null || newName === currentName) return;
    try {
      const r = await fetch("/api/applications/" + encodeURIComponent(applicationId), {
        method: "PUT",
        credentials: "include",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ application_name: newName }),
      });
      if (!r.ok) {
        const body = await r.json().catch(() => ({}));
        showError(listError, "Edit failed: " + (body.error || ("HTTP " + r.status)));
        return;
      }
      fetchApplications();
    } catch (e) {
      showError(listError, "Edit failed: " + e);
    }
  }

  async function onDelete(applicationId) {
    if (!window.confirm("Delete application " + applicationId + "?")) return;
    try {
      const r = await fetch("/api/applications/" + encodeURIComponent(applicationId), {
        method: "DELETE",
        credentials: "include",
        headers: { "Content-Type": "application/json" },
      });
      if (r.status !== 204 && !r.ok) {
        const body = await r.json().catch(() => ({}));
        showError(listError, "Delete failed: " + (body.error || ("HTTP " + r.status)));
        return;
      }
      fetchApplications();
    } catch (e) {
      showError(listError, "Delete failed: " + e);
    }
  }

  createForm.addEventListener("submit", async (event) => {
    event.preventDefault();
    clearError(createError);
    const idInput = document.getElementById("new-application-id");
    const nameInput = document.getElementById("new-application-name");
    const payload = {
      application_id: idInput.value.trim(),
      application_name: nameInput.value.trim(),
    };
    try {
      const r = await fetch("/api/applications", {
        method: "POST",
        credentials: "include",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      });
      if (r.status !== 201 && !r.ok) {
        const body = await r.json().catch(() => ({}));
        showError(createError, "Create failed: " + (body.error || ("HTTP " + r.status)));
        return;
      }
      idInput.value = "";
      nameInput.value = "";
      fetchApplications();
    } catch (e) {
      showError(createError, "Create failed: " + e);
    }
  });

  document.addEventListener("DOMContentLoaded", fetchApplications);
})();
