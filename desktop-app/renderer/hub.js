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

  // Hub responses are data, not markup. Keep every value that reaches an
  // innerHTML template either escaped text or an integer we derived locally.
  // In particular, do not trust an array-like `length` from a remote catalog.
  function boundedCount(value, max) {
    const count = Number(value);
    return Number.isSafeInteger(count) && count >= 0 ? Math.min(count, max) : 0;
  }

  function items(value) {
    return Array.isArray(value) ? value : [];
  }

  // #7: search is INTENT-AWARE and runs in the engine (fuzzy + intent expansion),
  // so the grid just renders whatever `categories` the host returned — no brittle
  // client-side exact-keyword filter, no "no matching categories" dead-end on
  // loose queries like "build a website".
  function renderGrid() {
    detail.hidden = true;
    grid.hidden = false;
    grid.innerHTML = "";
    const list = categories;
    if (list.length === 0) {
      grid.innerHTML = `<p class="muted">No categories matched — try a broader search.</p>`;
      return;
    }
    for (const rawCategory of list) {
      const c = rawCategory && typeof rawCategory === "object" ? rawCategory : {};
      const card = document.createElement("div");
      card.className = "card";
      card.innerHTML =
        `<div class="card-head"><span class="card-name">${esc(c.name)}</span>` +
        `<button class="add" title="Activate this package">+</button></div>` +
        `<p class="card-desc">${esc(c.description)}</p>` +
        // The engine sends `exampleRepos` (array), not `repoCount` (review #13/#22).
        `<div class="card-foot">${(() => {
          const count = boundedCount(items(c.exampleRepos).length, 10_000);
          return count ? `${count} example repos` : "browse →";
        })()}</div>`;
      card.querySelector(".add").addEventListener("click", (e) => {
        e.stopPropagation();
        vscode.postMessage({ type: "activate", slug: c.slug });
      });
      card.addEventListener("click", () => vscode.postMessage({ type: "openPackage", slug: c.slug }));
      grid.appendChild(card);
    }
  }

  function renderDetail(pkg) {
    pkg = pkg && typeof pkg === "object" ? pkg : {};
    current = pkg;
    grid.hidden = true;
    detail.hidden = false;
    const refs = items(pkg.references);
    const counts = pkg && typeof pkg.counts === "object" && pkg.counts ? pkg.counts : {};
    const ruleCount = boundedCount(counts.rules, 10_000);
    const skillCount = boundedCount(counts.skills, 10_000);
    detail.innerHTML = `
      <button id="back" class="back">← all categories</button>
      <h2>${esc(pkg.name)}</h2>
      <p class="muted">${esc(pkg.description)}</p>
      <div class="what">
        <strong>Activating injects (transparent steering):</strong>
        <ul>
          <li>${ruleCount} best-practice <b>rules</b> → your rules dir</li>
          <li>${skillCount} scaffold <b>skills</b> → your skills dir</li>
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
              (rawReference) => {
                const r = rawReference && typeof rawReference === "object" ? rawReference : {};
                return `<li><span class="rn">${esc(r.full_name)}</span>${r.license ? `<span class="lic">${esc(r.license)}</span>` : ""}</li>`;
              }
            )
            .join("")}
        </ul>
        <button id="support" class="ghost" ${refs.length ? "" : "disabled"}>⭐ Support these maintainers (${refs.length})</button>
      </div>`;
    detail.querySelector("#back").addEventListener("click", () => renderGrid());
    detail.querySelector("#activate").addEventListener("click", () =>
      vscode.postMessage({ type: "activate", slug: pkg.slug })
    );
    const sup = detail.querySelector("#support");
    if (sup && refs.length) {
      sup.addEventListener("click", () =>
        vscode.postMessage({
          type: "support",
          slug: pkg.slug,
          repos: refs.map((rawReference) => {
            const r = rawReference && typeof rawReference === "object" ? rawReference : {};
            return { full_name: r.full_name, html_url: r.html_url, license_spdx: r.license };
          }),
        })
      );
    }
  }

  // Debounce, then ask the ENGINE for intent-aware results (empty → full catalog).
  let searchTimer = null;
  searchEl.addEventListener("input", () => {
    if (searchTimer) clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      vscode.postMessage({ type: "search", q: searchEl.value });
    }, 180);
  });

  window.addEventListener("message", (event) => {
    const m = event.data;
    switch (m.type) {
      case "categories":
        categories = m.categories || [];
        statusEl.textContent = `${categories.length} domains · curated from public GitHub repos`;
        renderGrid();
        break;
      case "package":
        renderDetail(m.pkg);
        break;
      case "activated":
        statusEl.textContent = `✓ Activated ${m.name}: ${m.counts.rules} rules + ${m.counts.skills} skills injected`;
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
