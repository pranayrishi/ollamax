// Shim: the reused panel UI (media/main.js) calls `acquireVsCodeApi()` and
// expects a host that handles its postMessage() and replies via window messages.
// We provide that API; messages route to __forgeBridge (bridge.js), which talks
// to the local forge serve. State persists to localStorage.
(function () {
  const queue = [];
  window.acquireVsCodeApi = function () {
    return {
      postMessage: function (msg) {
        try {
          if (window.__forgeBridge) window.__forgeBridge.handle(msg);
          else queue.push(msg); // bridge not ready yet — flush below
        } catch (e) {
          // eslint-disable-next-line no-console
          console.error("forge bridge error", e);
        }
      },
      getState: function () {
        try {
          return JSON.parse(localStorage.getItem("forge.state") || "null");
        } catch (_) {
          return null;
        }
      },
      setState: function (s) {
        try {
          localStorage.setItem("forge.state", JSON.stringify(s));
        } catch (_) {}
        return s;
      },
    };
  };
  // Flush anything queued before bridge.js finished defining __forgeBridge.
  const flush = () => {
    if (window.__forgeBridge) {
      while (queue.length) window.__forgeBridge.handle(queue.shift());
    } else {
      setTimeout(flush, 15);
    }
  };
  flush();
})();
