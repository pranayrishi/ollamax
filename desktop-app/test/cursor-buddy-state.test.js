"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");

const {
  CURSOR_BUDDY_STATES,
  cursorBuddyBounds,
  cursorBuddyCueBounds,
  cursorBuddyState,
  normalizedPointToDisplayPoint,
} = require("../cursor-buddy-state");

test("cursor buddy accepts only its closed local state vocabulary", () => {
  const listening = cursorBuddyState("voice_listening");
  assert.deepEqual(listening, {
    key: "voice_listening",
    label: "Local voice",
    detail: "Listening · press shortcut again to stop",
    durationMs: 0,
  });
  assert.equal(cursorBuddyState("replicate this private search bar"), null);
  assert.equal(cursorBuddyState({ key: "voice_listening" }), null);
  assert.ok(Object.isFrozen(CURSOR_BUDDY_STATES));
});

test("cursor buddy stays within a display work area at normal and edge cursor positions", () => {
  const workArea = { x: -1440, y: 24, width: 1440, height: 876 };
  assert.deepEqual(
    cursorBuddyBounds({ x: -100, y: 100 }, workArea, { width: 232, height: 56 }),
    { x: -348, y: 120, width: 232, height: 56 }
  );
  assert.deepEqual(
    cursorBuddyBounds({ x: -2, y: 890 }, workArea, { width: 232, height: 56 }),
    { x: -250, y: 818, width: 232, height: 56 }
  );
});

test("cursor buddy bounds degrade safely for malformed points and small work areas", () => {
  const bounds = cursorBuddyBounds(null, { x: 4, y: 8, width: 80, height: 22 }, { width: 1, height: 1 });
  assert.deepEqual(bounds, { x: 4, y: 8, width: 120, height: 36 });
});

test("normalized POINT coordinates map to display geometry without moving the cursor", () => {
  const display = { x: -1440, y: 0, width: 1440, height: 900 };
  assert.deepEqual(normalizedPointToDisplayPoint({ x: 0, y: 0 }, display), { x: -1440, y: 0 });
  assert.deepEqual(normalizedPointToDisplayPoint({ x: 1, y: 1 }, display), { x: -1, y: 899 });
  assert.equal(normalizedPointToDisplayPoint({ x: 1.01, y: 0.5 }, display), null);
  assert.equal(normalizedPointToDisplayPoint({ x: 0.5, y: 0.5 }, { x: 0, y: 0, width: 0, height: 10 }), null);
});

test("point cue bounds keep a transient marker within the selected display", () => {
  const display = { x: -1440, y: 0, width: 1440, height: 900 };
  assert.deepEqual(
    cursorBuddyCueBounds({ x: -1400, y: 20 }, display),
    { x: -1432, y: 0, width: 264, height: 84 }
  );
  assert.deepEqual(
    cursorBuddyCueBounds({ x: -1, y: 899 }, display),
    { x: -264, y: 816, width: 264, height: 84 }
  );
});
