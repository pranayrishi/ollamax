// Shim: the reused panel UIs (media/main.js for chat, media/hub.js for the Hub)
// each call `acquireVsCodeApi()` and expect a host. We provide that API and
// dispatch every postMessage to ALL registered bridges (chat + hub). Each bridge
// ignores message types it doesn't own (their `default:` cases), so the two
// coexist in one window. Replies come back via window.postMessage, which each
// UI's `message` listener already filters by type.
(function () {
  const bridges = [];
  window.__forgeRegisterBridge = function (fn) {
    if (typeof fn === "function") bridges.push(fn);
  };
  function dispatch(msg) {
    for (const fn of bridges) {
      try {
        fn(msg);
      } catch (e) {
        // eslint-disable-next-line no-console
        console.error("forge bridge error", e);
      }
    }
  }
  window.acquireVsCodeApi = function () {
    return {
      postMessage: dispatch,
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
})();
