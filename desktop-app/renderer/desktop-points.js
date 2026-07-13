// Desktop-only bridge for finalized local POINT directives.  This runs before
// the shared chat UI and receives a response exactly once when its terminal
// `done` event arrives.  It does not observe streaming tokens, screenshots, or
// user input, and it has no pointer-control API.
(function () {
  "use strict";

  const parser = window.OllamaxPointDirectives;
  if (!parser || typeof parser.parsePointDirectives !== "function") return;

  window.__ollamaxFinalizeAssistantResponse = function finalizeAssistantResponse(answer) {
    const parsed = parser.parsePointDirectives(answer);
    const point = window.forgeNative && window.forgeNative.buddy && window.forgeNative.buddy.point;

    if (typeof point === "function") {
      for (const directive of parsed.directives) {
        // The main process independently validates every field, maps it to a
        // display, and draws a transient click-through cue.  Do not await it:
        // final chat rendering and speech should never be blocked by a cue.
        Promise.resolve(point(directive)).catch(() => {});
      }
    }
    return parsed.text;
  };
})();
