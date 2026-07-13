"use strict";

// The cursor companion intentionally accepts a small, closed set of local
// interaction states. It never receives a prompt, transcript, screenshot,
// file path, or arbitrary renderer text. The single exception is a separately
// validated, capped POINT label used by the transient visual cue below.
const CURSOR_BUDDY_STATES = Object.freeze({
  idle: Object.freeze({ label: "", detail: "", durationMs: 0 }),
  voice_starting: Object.freeze({ label: "Local voice", detail: "Starting microphone…", durationMs: 0 }),
  voice_listening: Object.freeze({ label: "Local voice", detail: "Listening · press shortcut again to stop", durationMs: 0 }),
  voice_transcribing: Object.freeze({ label: "Local voice", detail: "Transcribing on this device…", durationMs: 0 }),
  voice_done: Object.freeze({ label: "Local voice", detail: "Command ready", durationMs: 2_800 }),
  voice_error: Object.freeze({ label: "Local voice", detail: "Could not process that command", durationMs: 4_000 }),
  voice_unavailable: Object.freeze({ label: "Local voice", detail: "Whisper runtime is not available", durationMs: 4_500 }),
  voice_window_unavailable: Object.freeze({ label: "Local voice", detail: "Open Ollamax to use the shortcut", durationMs: 3_500 }),
  shortcut_unavailable: Object.freeze({ label: "Local voice", detail: "Shortcut is already in use", durationMs: 4_500 }),
  spatial_selecting: Object.freeze({ label: "Local region", detail: "Draw around the screen area", durationMs: 0 }),
  spatial_attached: Object.freeze({ label: "Local region", detail: "Selected region attached", durationMs: 3_200 }),
  spatial_cancelled: Object.freeze({ label: "Local region", detail: "Selection cancelled", durationMs: 1_800 }),
  spatial_error: Object.freeze({ label: "Local region", detail: "Could not select that region", durationMs: 4_000 }),
  pointing: Object.freeze({ label: "Local pointer", detail: "Look here", durationMs: 3_200 }),
});

function cursorBuddyState(name) {
  const key = typeof name === "string" ? name : "";
  const state = CURSOR_BUDDY_STATES[key];
  if (!state) return null;
  return { key, label: state.label, detail: state.detail, durationMs: state.durationMs };
}

function finiteNumber(value, fallback) {
  return Number.isFinite(value) ? value : fallback;
}

function clamp(value, minimum, maximum) {
  return Math.max(minimum, Math.min(value, maximum));
}

// Place a small non-interactive bubble by the cursor while keeping all of it
// inside the active display's work area. This is deliberately display-local:
// it does not inspect windows, accessibility trees, or any screen pixels.
function cursorBuddyBounds(cursorPoint, workArea, size = {}) {
  const width = Math.max(120, Math.round(finiteNumber(size.width, 232)));
  const height = Math.max(36, Math.round(finiteNumber(size.height, 56)));
  const x = finiteNumber(cursorPoint && cursorPoint.x, 0);
  const y = finiteNumber(cursorPoint && cursorPoint.y, 0);
  const areaX = finiteNumber(workArea && workArea.x, 0);
  const areaY = finiteNumber(workArea && workArea.y, 0);
  const areaWidth = Math.max(width, finiteNumber(workArea && workArea.width, width));
  const areaHeight = Math.max(height, finiteNumber(workArea && workArea.height, height));
  const right = areaX + areaWidth;
  const bottom = areaY + areaHeight;

  let left = x + 16;
  let top = y + 20;
  if (left + width > right) left = x - width - 16;
  if (top + height > bottom) top = y - height - 16;
  left = Math.max(areaX, Math.min(left, right - width));
  top = Math.max(areaY, Math.min(top, bottom - height));

  return {
    x: Math.round(left),
    y: Math.round(top),
    width,
    height,
  };
}

// Map an explicit normalized POINT coordinate to a display-local DIP point.
// This is geometry only: it neither queries UI elements nor changes the OS
// pointer. `width - 1` / `height - 1` make the inclusive 1.0 endpoint land on
// the final addressable display coordinate rather than just outside it.
function normalizedPointToDisplayPoint(normalizedPoint, displayBounds) {
  if (
    !normalizedPoint ||
    !Number.isFinite(normalizedPoint.x) ||
    !Number.isFinite(normalizedPoint.y) ||
    normalizedPoint.x < 0 ||
    normalizedPoint.x > 1 ||
    normalizedPoint.y < 0 ||
    normalizedPoint.y > 1 ||
    !displayBounds ||
    !Number.isFinite(displayBounds.x) ||
    !Number.isFinite(displayBounds.y) ||
    !Number.isFinite(displayBounds.width) ||
    !Number.isFinite(displayBounds.height) ||
    displayBounds.width <= 0 ||
    displayBounds.height <= 0
  ) {
    return null;
  }

  const width = Math.max(1, Math.round(displayBounds.width));
  const height = Math.max(1, Math.round(displayBounds.height));
  return {
    x: Math.round(displayBounds.x + normalizedPoint.x * (width - 1)),
    y: Math.round(displayBounds.y + normalizedPoint.y * (height - 1)),
  };
}

// Keep a point-target cue entirely inside a display. Unlike the regular
// cursor-status bubble, this uses the display's full bounds so a normalized
// coordinate near a menu bar or dock does not get clipped by its work area.
function cursorBuddyCueBounds(targetPoint, displayBounds, size = {}) {
  const width = Math.max(180, Math.round(finiteNumber(size.width, 264)));
  const height = Math.max(64, Math.round(finiteNumber(size.height, 84)));
  const areaX = finiteNumber(displayBounds && displayBounds.x, 0);
  const areaY = finiteNumber(displayBounds && displayBounds.y, 0);
  const areaWidth = Math.max(width, finiteNumber(displayBounds && displayBounds.width, width));
  const areaHeight = Math.max(height, finiteNumber(displayBounds && displayBounds.height, height));
  const right = areaX + areaWidth;
  const bottom = areaY + areaHeight;
  const targetX = finiteNumber(targetPoint && targetPoint.x, areaX);
  const targetY = finiteNumber(targetPoint && targetPoint.y, areaY);

  return {
    x: Math.round(clamp(targetX - 32, areaX, right - width)),
    y: Math.round(clamp(targetY - 34, areaY, bottom - height)),
    width,
    height,
  };
}

module.exports = {
  CURSOR_BUDDY_STATES,
  cursorBuddyBounds,
  cursorBuddyCueBounds,
  cursorBuddyState,
  normalizedPointToDisplayPoint,
};
