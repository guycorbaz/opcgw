// Story 9-6: vanilla JS controller for /commands.html.
// No SPA framework; no build step; no npm install.
//
// All mutating fetches set Content-Type: application/json and rely
// on the browser to attach Origin (the gateway's CSRF middleware
// checks against [web].allowed_origins).

(function () {
  const container = document.getElementById("applications-container");
  const listError = document.getElementById("list-error");
  const editModal = document.getElementById("edit-modal");
  const editForm = document.getElementById("edit-form");
  const editError = document.getElementById("edit-error");

  let editModalLoading = false;

  function escapeHtml(s) {
    return String(s)
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;")
      .replace(/'/g, "&#39;")
      .replace(/`/g, "&#96;");
  }

  function showError(el, msg) {
    el.textContent = msg;
    el.hidden = false;
  }
  function clearError(el) {
    el.textContent = "";
    el.hidden = true;
  }

  // Story 9-5 iter-2 L2 pattern: treat Content-Length: 0 as no body.
  async function fetchJson(url, options) {
    const res = await fetch(url, options || {});
    if (!res.ok) {
      let body = {};
      try {
        body = await res.json();
      } catch (_) {
        // empty or non-JSON body
      }
      const err = new Error(body.error || ("HTTP " + res.status));
      err.status = res.status;
      err.body = body;
      throw err;
    }
    const lengthHdr = res.headers.get("content-length");
    if (lengthHdr === "0" || res.status === 204) {
      return null;
    }
    return res.json();
  }

  async function loadAll() {
    clearError(listError);
    container.innerHTML = "<p class=\"loading\">Loading&hellip;</p>";
    try {
      const data = await fetchJson("/api/applications", { credentials: "include" });
      const apps = data.applications || [];
      if (apps.length === 0) {
        container.innerHTML = "<p>No applications configured. Create one via the Applications page.</p>";
        return;
      }
      container.innerHTML = "";
      for (const app of apps) {
        await renderApplication(app);
      }
    } catch (e) {
      container.innerHTML = "";
      showError(listError, "Failed to load applications: " + (e.message || e));
    }
  }

  async function renderApplication(app) {
    const section = document.createElement("section");
    section.className = "application-section";
    section.setAttribute("data-application-id", app.application_id);
    section.innerHTML =
      "<h2>" + escapeHtml(app.application_name) +
      " <small>(" + escapeHtml(app.application_id) + ")</small></h2>" +
      "<div class=\"devices-container\">Loading devices&hellip;</div>";
    container.appendChild(section);

    try {
      const data = await fetchJson(
        "/api/applications/" + encodeURIComponent(app.application_id) + "/devices",
        { credentials: "include" }
      );
      const devContainer = section.querySelector(".devices-container");
      devContainer.innerHTML = "";
      const devices = data.devices || [];
      if (devices.length === 0) {
        devContainer.innerHTML = "<p><em>No devices under this application.</em></p>";
        return;
      }
      for (const dev of devices) {
        await renderDevice(app.application_id, dev, devContainer);
      }
    } catch (e) {
      section.querySelector(".devices-container").innerHTML =
        "<p class=\"error-banner\">Failed to load devices: " + escapeHtml(e.message || String(e)) + "</p>";
    }
  }

  async function renderDevice(applicationId, dev, parent) {
    const block = document.createElement("div");
    block.className = "device-section";
    block.setAttribute("data-device-id", dev.device_id);
    block.innerHTML =
      "<h3>" + escapeHtml(dev.device_name) +
      " <small>(" + escapeHtml(dev.device_id) + ")</small></h3>" +
      "<div class=\"commands-table-container\">Loading commands&hellip;</div>";
    parent.appendChild(block);

    await refreshCommandsTable(applicationId, dev.device_id, block);
    renderCreateForm(applicationId, dev.device_id, block);
  }

  async function refreshCommandsTable(applicationId, deviceId, parent) {
    const tableContainer = parent.querySelector(".commands-table-container");
    tableContainer.innerHTML = "Loading commands&hellip;";
    try {
      const data = await fetchJson(
        "/api/applications/" + encodeURIComponent(applicationId) +
        "/devices/" + encodeURIComponent(deviceId) + "/commands",
        { credentials: "include" }
      );
      const commands = data.commands || [];
      if (commands.length === 0) {
        tableContainer.innerHTML = "<p><em>No commands configured for this device.</em></p>";
        return;
      }
      let html = "<table class=\"commands\"><thead><tr>" +
        "<th>command_id</th><th>command_name</th><th>command_port</th>" +
        "<th>command_confirmed</th><th>Actions</th></tr></thead><tbody>";
      for (const c of commands) {
        html +=
          "<tr>" +
            "<td>" + c.command_id + "</td>" +
            "<td>" + escapeHtml(c.command_name) + "</td>" +
            "<td>" + c.command_port + "</td>" +
            "<td>" + (c.command_confirmed ? "true" : "false") + "</td>" +
            "<td class=\"actions\">" +
              "<button class=\"btn-edit\" data-app=\"" + escapeHtml(applicationId) +
              "\" data-dev=\"" + escapeHtml(deviceId) +
              "\" data-cmd=\"" + c.command_id + "\">Edit</button>" +
              "<button class=\"btn-delete\" data-app=\"" + escapeHtml(applicationId) +
              "\" data-dev=\"" + escapeHtml(deviceId) +
              "\" data-cmd=\"" + c.command_id + "\">Delete</button>" +
            "</td>" +
          "</tr>";
      }
      html += "</tbody></table>";
      tableContainer.innerHTML = html;

      tableContainer.querySelectorAll(".btn-edit").forEach((btn) => {
        btn.addEventListener("click", () =>
          openEditModal(btn.dataset.app, btn.dataset.dev, parseInt(btn.dataset.cmd, 10))
        );
      });
      tableContainer.querySelectorAll(".btn-delete").forEach((btn) => {
        btn.addEventListener("click", () =>
          onDelete(btn.dataset.app, btn.dataset.dev, parseInt(btn.dataset.cmd, 10))
        );
      });
    } catch (e) {
      tableContainer.innerHTML =
        "<p class=\"error-banner\">Failed to load commands: " + escapeHtml(e.message || String(e)) + "</p>";
    }
  }

  function renderCreateForm(applicationId, deviceId, parent) {
    const form = document.createElement("form");
    form.className = "crud-form";
    form.innerHTML =
      "<h4>Create command</h4>" +
      "<label>command_id <input type=\"number\" min=\"1\" required name=\"command_id\"></label>" +
      "<label>command_name <input type=\"text\" required name=\"command_name\"></label>" +
      "<label>command_port (LoRaWAN f_port, 1&ndash;223) <input type=\"number\" min=\"1\" max=\"223\" required name=\"command_port\"></label>" +
      "<label><input type=\"checkbox\" name=\"command_confirmed\"> Confirmed downlink</label>" +
      "<div class=\"create-error error-banner\" hidden></div>" +
      "<button type=\"submit\" class=\"btn-add\">Create command</button>";
    parent.appendChild(form);

    const createError = form.querySelector(".create-error");
    form.addEventListener("submit", async (event) => {
      event.preventDefault();
      clearError(createError);
      const fd = new FormData(form);
      const payload = {
        command_id: parseInt(fd.get("command_id"), 10),
        command_name: String(fd.get("command_name") || "").trim(),
        command_port: parseInt(fd.get("command_port"), 10),
        command_confirmed: fd.get("command_confirmed") === "on",
      };
      try {
        await fetchJson(
          "/api/applications/" + encodeURIComponent(applicationId) +
          "/devices/" + encodeURIComponent(deviceId) + "/commands",
          {
            method: "POST",
            credentials: "include",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify(payload),
          }
        );
        form.reset();
        await refreshCommandsTable(applicationId, deviceId, parent);
      } catch (e) {
        showError(createError, "Create failed: " + (e.message || e));
      }
    });
  }

  async function openEditModal(applicationId, deviceId, commandId) {
    // Story 9-5 iter-2 M4 pattern: try/finally around the loading
    // flag so a synchronous DOM-null deref above the inner try
    // doesn't leave the modal permanently inert.
    if (editModalLoading) return;
    editModalLoading = true;
    try {
      clearError(editError);
      try {
        const data = await fetchJson(
          "/api/applications/" + encodeURIComponent(applicationId) +
          "/devices/" + encodeURIComponent(deviceId) +
          "/commands/" + commandId,
          { credentials: "include" }
        );
        document.getElementById("edit-application-id").value = applicationId;
        document.getElementById("edit-device-id").value = deviceId;
        document.getElementById("edit-command-id").value = String(commandId);
        document.getElementById("edit-command-id-display").textContent = String(commandId);
        document.getElementById("edit-command-name").value = data.command_name;
        document.getElementById("edit-command-port").value = data.command_port;
        document.getElementById("edit-command-confirmed").checked = !!data.command_confirmed;
        editModal.showModal();
      } catch (e) {
        showError(listError, "Could not load command for editing: " + (e.message || e));
      }
    } finally {
      editModalLoading = false;
    }
  }

  function closeEditModal() {
    editModal.close();
    editModalLoading = false;
  }

  editForm.addEventListener("submit", async (event) => {
    event.preventDefault();
    clearError(editError);
    const applicationId = document.getElementById("edit-application-id").value;
    const deviceId = document.getElementById("edit-device-id").value;
    const commandId = parseInt(document.getElementById("edit-command-id").value, 10);
    const payload = {
      command_name: document.getElementById("edit-command-name").value.trim(),
      command_port: parseInt(document.getElementById("edit-command-port").value, 10),
      command_confirmed: document.getElementById("edit-command-confirmed").checked,
    };
    try {
      await fetchJson(
        "/api/applications/" + encodeURIComponent(applicationId) +
        "/devices/" + encodeURIComponent(deviceId) +
        "/commands/" + commandId,
        {
          method: "PUT",
          credentials: "include",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(payload),
        }
      );
      closeEditModal();
      await loadAll();
    } catch (e) {
      showError(editError, "Edit failed: " + (e.message || e));
    }
  });

  document.getElementById("edit-cancel").addEventListener("click", closeEditModal);

  async function onDelete(applicationId, deviceId, commandId) {
    if (!window.confirm("Delete command " + commandId + "?")) return;
    try {
      await fetchJson(
        "/api/applications/" + encodeURIComponent(applicationId) +
        "/devices/" + encodeURIComponent(deviceId) +
        "/commands/" + commandId,
        {
          method: "DELETE",
          credentials: "include",
          headers: { "Content-Type": "application/json" },
        }
      );
      await loadAll();
    } catch (e) {
      showError(listError, "Delete failed: " + (e.message || e));
    }
  }

  document.addEventListener("DOMContentLoaded", loadAll);
})();
