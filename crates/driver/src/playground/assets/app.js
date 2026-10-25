// Zap Playground front-end (I#238). No framework: a textarea editor that POSTs
// the snippet to /run and renders the JSON result. Share links are stateless —
// the snippet is encoded in the URL fragment, so nothing is stored server-side.
"use strict";

(function () {
  const codeEl = document.getElementById("code");
  const runBtn = document.getElementById("run");
  const shareBtn = document.getElementById("share");
  const shareNote = document.getElementById("sharenote");
  const statusEl = document.getElementById("status");
  const stdoutEl = document.getElementById("stdout");
  const diagsEl = document.getElementById("diagnostics");
  const runtimeEl = document.getElementById("runtime");
  const messageEl = document.getElementById("message");

  // --- stateless share links: snippet lives in the URL fragment -------------
  function encode(text) {
    const b64 = btoa(unescape(encodeURIComponent(text)));
    return b64.replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  }
  function decode(s) {
    let b64 = s.replace(/-/g, "+").replace(/_/g, "/");
    while (b64.length % 4) b64 += "=";
    return decodeURIComponent(escape(atob(b64)));
  }
  function loadFromHash() {
    const m = location.hash.match(/(?:^#|&)code=([^&]+)/);
    if (!m) return;
    try {
      codeEl.value = decode(m[1]);
    } catch (_e) {
      /* ignore a malformed fragment */
    }
  }

  shareBtn.addEventListener("click", function () {
    const url =
      location.origin + location.pathname + "#code=" + encode(codeEl.value);
    history.replaceState(null, "", url);
    const done = () => {
      shareNote.textContent = "link copied";
      setTimeout(() => (shareNote.textContent = ""), 2000);
    };
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(url).then(done, done);
    } else {
      done();
    }
  });

  // --- running --------------------------------------------------------------
  const STATUS_LABEL = {
    ok: "ok",
    error: "compile error",
    runtime_error: "runtime error",
    timeout: "timed out",
    output_limit: "output limit",
    crashed: "crashed",
    internal: "server error",
    busy: "busy",
  };

  function clearOutput() {
    stdoutEl.textContent = "";
    diagsEl.textContent = "";
    runtimeEl.textContent = "";
    messageEl.textContent = "";
  }

  function render(result) {
    const status = result.status || "internal";
    statusEl.textContent = STATUS_LABEL[status] || status;
    statusEl.className = "status status-" + status;

    stdoutEl.textContent = result.stdout || "";
    if (result.truncated) {
      stdoutEl.textContent += "\n… (output truncated)";
    }

    diagsEl.textContent = "";
    (result.diagnostics || []).forEach(function (d) {
      const line = document.createElement("div");
      line.className = "diag";
      line.textContent = d;
      diagsEl.appendChild(line);
    });

    runtimeEl.textContent = result.runtime_error
      ? "Uncaught exception: " + result.runtime_error
      : "";
    messageEl.textContent = result.message || "";
  }

  function run() {
    clearOutput();
    statusEl.textContent = "running…";
    statusEl.className = "status status-running";
    runBtn.disabled = true;
    fetch("/run", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ code: codeEl.value }),
    })
      .then((r) => r.json())
      .then(render)
      .catch(function (e) {
        render({ status: "internal", message: String(e) });
      })
      .finally(function () {
        runBtn.disabled = false;
      });
  }

  runBtn.addEventListener("click", run);

  // Ctrl/Cmd-Enter runs; Tab inserts four spaces instead of leaving the field.
  codeEl.addEventListener("keydown", function (e) {
    if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
      e.preventDefault();
      run();
    } else if (e.key === "Tab") {
      e.preventDefault();
      const s = codeEl.selectionStart;
      const eN = codeEl.selectionEnd;
      codeEl.value = codeEl.value.slice(0, s) + "    " + codeEl.value.slice(eN);
      codeEl.selectionStart = codeEl.selectionEnd = s + 4;
    }
  });

  loadFromHash();
})();
