// App shell: a thin left rail that switches the main area between the Chat view
// and the Central Hub view (separate panels, single window). External file
// because the app CSP is `script-src 'self'` (no inline scripts).
(function () {
  const VIEWS = ["chat", "hub", "ide"];
  function show(view) {
    VIEWS.forEach((v) => {
      const el = document.getElementById(v + "-view");
      if (el) el.hidden = v !== view;
    });
    document.querySelectorAll("#rail .rail-btn").forEach((b) =>
      b.classList.toggle("active", b.getAttribute("data-view") === view)
    );
  }
  window.addEventListener("DOMContentLoaded", () => {
    document.querySelectorAll("#rail .rail-btn").forEach((b) =>
      b.addEventListener("click", () => show(b.getAttribute("data-view")))
    );
    show("chat");
  });
})();
