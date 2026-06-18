// App shell: a thin left rail that switches the main area between the Chat view
// and the Central Hub view (separate panels, single window). External file
// because the app CSP is `script-src 'self'` (no inline scripts).
(function () {
  function show(view) {
    const chat = document.getElementById("chat-view");
    const hubv = document.getElementById("hub-view");
    if (chat) chat.hidden = view !== "chat";
    if (hubv) hubv.hidden = view !== "hub";
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
