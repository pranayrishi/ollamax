// Full-screen lasso overlay.  It only receives a screenshot already captured
// by the main process after an explicit click, and it returns pointer points to
// that same process; no pixels are sent through a network or written to disk.
(function () {
  "use strict";
  const image = document.querySelector("#capture");
  const canvas = document.querySelector("#lasso");
  const context = canvas.getContext("2d");
  // Keep the renderer bounded too. The privileged main process independently
  // rejects more than this many samples, but an unusually long pointer hold
  // should not let this visual-only overlay accumulate an unbounded path first.
  const MAX_LASSO_POINTS = 2048;
  let init = null;
  let points = [];
  let drawing = false;

  function appendPoint(point) {
    if (points.length < MAX_LASSO_POINTS) {
      points.push(point);
    } else {
      // Preserve the most recent endpoint for the crop while keeping the
      // payload valid for the main-process cap.
      points[points.length - 1] = point;
    }
  }

  function resizeCanvas() {
    const ratio = Math.max(1, window.devicePixelRatio || 1);
    canvas.width = Math.max(1, Math.round(window.innerWidth * ratio));
    canvas.height = Math.max(1, Math.round(window.innerHeight * ratio));
    canvas.style.width = `${window.innerWidth}px`;
    canvas.style.height = `${window.innerHeight}px`;
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    redraw();
  }

  function pointFromEvent(event) {
    return {
      x: Math.max(0, Math.min(window.innerWidth, event.clientX)),
      y: Math.max(0, Math.min(window.innerHeight, event.clientY)),
    };
  }

  function redraw() {
    context.clearRect(0, 0, window.innerWidth, window.innerHeight);
    if (!points.length) return;
    context.fillStyle = "rgba(245, 158, 11, .16)";
    context.strokeStyle = "#fbbf24";
    context.lineWidth = 2;
    context.lineJoin = "round";
    context.lineCap = "round";
    context.beginPath();
    context.moveTo(points[0].x, points[0].y);
    for (let i = 1; i < points.length; i += 1) context.lineTo(points[i].x, points[i].y);
    if (points.length > 2) {
      context.closePath();
      context.fill();
    }
    context.stroke();
  }

  function cancel() {
    if (init) window.spatialNative.cancel({ sessionId: init.sessionId, displayId: init.displayId });
  }

  function finish(event) {
    if (!drawing || !init) return;
    drawing = false;
    appendPoint(pointFromEvent(event));
    redraw();
    window.spatialNative.complete({ sessionId: init.sessionId, displayId: init.displayId, points });
  }

  document.addEventListener("pointerdown", (event) => {
    if (!init || event.button !== 0) return;
    drawing = true;
    points = [pointFromEvent(event)];
    document.body.setPointerCapture && document.body.setPointerCapture(event.pointerId);
    redraw();
  });
  document.addEventListener("pointermove", (event) => {
    if (!drawing) return;
    const point = pointFromEvent(event);
    const previous = points[points.length - 1];
    if (!previous || Math.hypot(previous.x - point.x, previous.y - point.y) >= 1) {
      appendPoint(point);
      redraw();
    }
  });
  document.addEventListener("pointerup", finish);
  document.addEventListener("pointercancel", cancel);
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") cancel();
  });
  window.addEventListener("resize", resizeCanvas);
  window.spatialNative.onInit((payload) => {
    init = payload;
    image.src = payload.image;
    resizeCanvas();
  });
})();
