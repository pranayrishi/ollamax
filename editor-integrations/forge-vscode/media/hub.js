// @ts-check
/* Hub webview. Renders category cards with "+" to activate a package, a detail
 * view showing exactly what a package injects (rules/skills/references + license),
 * and an explicit opt-in "Support these maintainers" action. No network here —
 * the extension host fetches the catalog. */
(function () {
  "use strict";
  const vscode = acquireVsCodeApi();
  const $ = (s) => document.querySelector(s);
  const grid = $("#grid");
  const detail = $("#detail");
  const statusEl = $("#status");
  const searchEl = $("#search");

  let categories = [];
  let current = null;

  function esc(s) {
    return String(s == null ? "" : s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  }

  function renderGrid(filter) {
    detail.hidden = true;
    grid.hidden = false;
    grid.innerHTML = "";
    const f = (filter || "").toLowerCase();
    const list = categories.filter(
      (c) => !f || c.name.toLowerCase().includes(f) || (c.topics || []).some((t) => t.includes(f))
    );
    if (list.length === 0) {
      grid.innerHTML = `<p class="muted">No matching categories.</p>`;
      return;
    }
    for (const c of list) {
      const card = document.createElement("div");
      card.className = "card";
      card.innerHTML =
        `<div class="card-head"><span class="card-name">${esc(c.name)}</span>` +
        `<button class="add" title="Activate this package">+</button></div>` +
        `<p class="card-desc">${esc(c.description)}</p>` +
        `<div class="card-foot">${c.repoCount ? c.repoCount + " curated repos" : "catalog refreshing…"}</div>`;
      card.querySelector(".add").addEventListener("click", (e) => {
        e.stopPropagation();
        vscode.postMessage({ type: "activate", slug: c.slug });
      });
      card.addEventListener("click", () => vscode.postMessage({ type: "openPackage", slug: c.slug }));
      grid.appendChild(card);
    }
  }

  function renderDetail(pkg) {
    current = pkg;
    grid.hidden = true;
    detail.hidden = false;
    const refs = pkg.references || [];
    detail.innerHTML = `
      <button id="back" class="back">← all categories</button>
      <h2>${esc(pkg.name)}</h2>
      <p class="muted">${esc(pkg.description)}</p>
      <div class="what">
        <strong>Activating injects (transparent steering):</strong>
        <ul>
          <li>${pkg.counts.rules} best-practice <b>rules</b> → your rules dir</li>
          <li>${pkg.counts.skills} scaffold <b>skills</b> → your skills dir</li>
          <li>${refs.length} curated <b>references</b> (links only)</li>
        </ul>
        <p class="muted small">Generic, license-safe conventions — not copied source code. Reversible (delete the files).</p>
      </div>
      <button id="activate" class="primary">+ Activate package</button>
      <div class="support">
        <h3>Support these maintainers (optional)</h3>
        <p class="muted small">Star the repos behind this package to credit their maintainers. Opt-in, in your browser, reviewed by you. Never automatic.</p>
        <ul class="reflist">
          ${refs
            .map(
              (r) =>
                `<li><span class="rn">${esc(r.full_name)}</span><span class="lic">${esc(r.license || "no license")}</span></li>`
            )
            .join("")}
        </ul>
        <button id="support" class="ghost" ${refs.length ? "" : "disabled"}>⭐ Support these maintainers (${refs.length})</button>
      </div>`;
    detail.querySelector("#back").addEventListener("click", () => renderGrid(searchEl.value));
    detail.querySelector("#activate").addEventListener("click", () =>
      vscode.postMessage({ type: "activate", slug: pkg.slug })
    );
    const sup = detail.querySelector("#support");
    if (sup && refs.length) {
      sup.addEventListener("click", () =>
        vscode.postMessage({
          type: "support",
          slug: pkg.slug,
          repos: refs.map((r) => ({ full_name: r.full_name, html_url: r.html_url, license_spdx: r.license })),
        })
      );
    }
  }

  searchEl.addEventListener("input", () => {
    if (detail.hidden) renderGrid(searchEl.value);
  });

  window.addEventListener("message", (event) => {
    const m = event.data;
    switch (m.type) {
      case "categories":
        categories = m.categories || [];
        statusEl.textContent = `${categories.length} domains · curated from public GitHub repos`;
        renderGrid(searchEl.value);
        break;
      case "package":
        renderDetail(m.pkg);
        break;
      case "activated":
        statusEl.textContent = `✓ Activated ${m.name}: ${m.counts.rules} rules + ${m.counts.skills} skills injected`;
        break;
      case "needsServer":
        statusEl.innerHTML =
          "Set <code>forge.accountServer</code> in Settings to load the Hub catalog.";
        grid.innerHTML = "";
        break;
      case "needsSignIn":
        statusEl.textContent = "Sign in with GitHub (chat panel) to support maintainers.";
        break;
      case "error":
        statusEl.textContent = "⚠ " + m.message;
        break;
      default:
        break;
    }
  });

  vscode.postMessage({ type: "ready" });
})();
