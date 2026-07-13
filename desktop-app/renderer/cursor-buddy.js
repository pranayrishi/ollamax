// A visual-only companion. The main process supplies only labels from its
// closed local state set; this renderer never sees voice text or model output.
(function () {
  "use strict";
  const buddy = document.querySelector("#buddy");
  const label = document.querySelector("#label");
  const detail = document.querySelector("#detail");
  const targetCue = document.querySelector("#target-cue");
  if (!buddy || !label || !detail || !window.cursorBuddyNative) return;

  window.cursorBuddyNative.onState((payload) => {
    const nextLabel = String((payload && payload.label) || "").slice(0, 48);
    const nextDetail = String((payload && payload.detail) || "").slice(0, 96);
    const cue = payload && payload.cue;
    const hasCue = !!(
      targetCue &&
      cue &&
      typeof cue.x === "number" &&
      typeof cue.y === "number" &&
      Number.isFinite(cue.x) &&
      Number.isFinite(cue.y)
    );
    label.textContent = nextLabel;
    detail.textContent = nextDetail;
    buddy.hidden = !nextLabel && !nextDetail;
    buddy.classList.toggle("pointing", hasCue);
    if (targetCue) {
      if (hasCue) {
        // Values were calculated by the main process. Clamp again before
        // assigning CSS so this unprivileged display surface stays bounded.
        targetCue.style.left = `${Math.max(0, Math.min(500, Math.round(cue.x)))}px`;
        targetCue.style.top = `${Math.max(0, Math.min(500, Math.round(cue.y)))}px`;
        targetCue.hidden = false;
      } else {
        targetCue.hidden = true;
      }
    }
  });
})();
