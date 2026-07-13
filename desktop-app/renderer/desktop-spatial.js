// Desktop-only bridge from the explicit region button into the sandboxed
// screen-selection overlay. The resulting cropped image is sent through the
// existing local chat context protocol, never to a third-party service.
(function () {
  "use strict";
  const button = document.querySelector("#spatial-select");
  const status = document.querySelector("#voice-status");
  if (!button || !window.forgeNative || !window.forgeNative.spatial) return;
  const buddy = window.forgeNative.buddy;
  const reportBuddy = (state) => {
    if (!buddy || typeof buddy.setState !== "function") return;
    Promise.resolve(buddy.setState(state)).catch(() => {});
  };
  const setStatus = (text, bad) => {
    if (!status) return;
    status.textContent = text || "";
    status.classList.toggle("bad", !!bad);
  };
  button.addEventListener("click", async () => {
    if (button.disabled) return;
    button.disabled = true;
    setStatus("Select a region on screen. The full screenshot is never saved; only the selected crop is kept in memory for this local request.");
    reportBuddy("spatial_selecting");
    try {
      const result = await window.forgeNative.spatial.select();
      if (!result || !result.ok) {
        setStatus((result && result.error) || "Screen-region selection failed.", true);
        const message = String((result && result.error) || "").toLowerCase();
        reportBuddy(message.includes("cancel") ? "spatial_cancelled" : "spatial_error");
        return;
      }
      window.postMessage({ type: "context", items: [result.item] }, "*");
      setStatus("Selected screen region attached as local visual context. Any Agent/Team visual brief is not added to Ollamax memory or replay logs.");
      reportBuddy("spatial_attached");
    } catch (error) {
      setStatus(String((error && error.message) || error), true);
      reportBuddy("spatial_error");
    } finally {
      button.disabled = false;
    }
  });
})();
