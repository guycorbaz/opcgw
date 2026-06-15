// opcgw config export / import — Story F-4.
//
// Export is a plain authenticated <a download> link to GET /api/config/export
// (the browser reuses the page's cached Basic-auth credentials). Import reads
// the chosen file CLIENT-SIDE (FileReader) and POSTs { toml } to
// POST /api/config/import — there is no multipart upload because the CSRF
// middleware requires Content-Type: application/json. A successful import is
// STAGED; the apply-bar (apply-bar.js) picks up the pending change on its next
// poll, and the operator clicks "Apply changes" to activate. No build step.

(function () {
  "use strict";

  var fileInput = document.getElementById("import-file");
  var importBtn = document.getElementById("import-btn");
  var statusEl = document.getElementById("import-status");
  if (!importBtn || !fileInput || !statusEl) {
    return; // not on the config page
  }

  function showStatus(message, kind) {
    statusEl.textContent = message;
    // Reuse the shared .banner component (F-1): is-ok / is-warn / is-error.
    statusEl.className = "banner" + (kind ? " " + kind : "");
    statusEl.classList.remove("hidden");
  }

  importBtn.addEventListener("click", function () {
    var file = fileInput.files && fileInput.files[0];
    if (!file) {
      showStatus("Choose a .toml file to import first.", "is-warn");
      return;
    }

    var reader = new FileReader();
    reader.onerror = function () {
      showStatus("Could not read the selected file.", "is-error");
    };
    reader.onload = function () {
      var text = String(reader.result || "");
      importBtn.disabled = true;
      showStatus("Validating and staging the imported configuration…", null);

      fetch("/api/config/import", {
        method: "POST",
        credentials: "same-origin",
        cache: "no-store",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ toml: text }),
      })
        .then(function (resp) {
          return resp
            .json()
            .catch(function () {
              return {};
            })
            .then(function (body) {
              return { status: resp.status, body: body };
            });
        })
        .then(function (r) {
          importBtn.disabled = false;
          if (r.status === 202) {
            showStatus(
              "Configuration imported and staged. Click “Apply changes” below to activate it.",
              "is-ok"
            );
          } else if (r.status === 401) {
            showStatus(
              "Session expired or credentials no longer accepted. Reload the page.",
              "is-error"
            );
          } else {
            var reason =
              (r.body && (r.body.reason || r.body.error)) || "HTTP " + r.status;
            var hint = r.body && r.body.hint ? " — " + r.body.hint : "";
            showStatus("Import rejected (" + reason + ")" + hint, "is-error");
          }
        })
        .catch(function () {
          importBtn.disabled = false;
          showStatus("Import failed (network error). Check the gateway connection.", "is-error");
        });
    };
    reader.readAsText(file);
  });
})();
