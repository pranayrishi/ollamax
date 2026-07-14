// Unit tests for the companion's pure protocol logic.
// Run with:  npm test  (in desktop-app/), or
//            node --test desktop-app/companion/protocol.test.js
"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const {
  parseCompanionReply,
  scalePointToDisplay,
  strokeBoundingBox,
  sanitizeTranscript,
  buildCompanionSystemPrompt,
} = require("./protocol");

test("parses a plain POINT tag with label", () => {
  const r = parseCompanionReply(
    "see that source control menu up top? click it. [POINT:285,11:source control]"
  );
  assert.equal(r.spokenText, "see that source control menu up top? click it.");
  assert.deepEqual(r.point, { x: 285, y: 11, label: "source control", screenNumber: null });
  assert.equal(r.task, null);
});

test("parses POINT:none as no pointing", () => {
  const r = parseCompanionReply("html is the skeleton of every web page. [POINT:none]");
  assert.equal(r.point, null);
  assert.equal(r.spokenText, "html is the skeleton of every web page.");
});

test("parses a screen-qualified POINT tag", () => {
  const r = parseCompanionReply("that's on your other monitor. [POINT:400,300:terminal:screen2]");
  assert.deepEqual(r.point, { x: 400, y: 300, label: "terminal", screenNumber: 2 });
});

test("parses POINT followed by TASK (spatial replicate flow)", () => {
  const r = parseCompanionReply(
    "nice pick — i'll have the agent build that. [POINT:none][TASK: replicate the circled search bar as a reusable component]"
  );
  assert.equal(r.spokenText, "nice pick — i'll have the agent build that.");
  assert.equal(r.point, null);
  assert.equal(r.task, "replicate the circled search bar as a reusable component");
});

test("TASK alone parses and strips", () => {
  const r = parseCompanionReply("on it. [TASK: add a dark-mode toggle to the settings page]");
  assert.equal(r.task, "add a dark-mode toggle to the settings page");
  assert.equal(r.spokenText, "on it.");
});

test("coordinates quoted mid-sentence are left alone", () => {
  const r = parseCompanionReply("the tag format is [POINT:10,20:label] which i can use. ok?");
  assert.equal(r.point, null);
  assert.ok(r.spokenText.includes("[POINT:10,20:label]"));
});

test("strips <think> blocks from reasoning models", () => {
  const r = parseCompanionReply(
    "<think>the user wants the save button which is at 100,50</think>hit save up top. [POINT:100,50:save]"
  );
  assert.equal(r.spokenText, "hit save up top.");
  assert.deepEqual(r.point, { x: 100, y: 50, label: "save", screenNumber: null });
});

test("scales screenshot pixels to display DIP with clamping", () => {
  const bounds = { x: 0, y: 0, width: 1512, height: 982 };
  const p = scalePointToDisplay({ x: 1280, y: 400 }, 2560, 1600, bounds);
  assert.equal(Math.round(p.x), 756);
  assert.equal(Math.round(p.y), Math.round((400 * 982) / 1600));
  // Out-of-range coordinates clamp to the image instead of flying off-screen.
  const clamped = scalePointToDisplay({ x: 99999, y: -5 }, 2560, 1600, bounds);
  assert.equal(clamped.x, bounds.width);
  assert.equal(clamped.y, 0);
});

test("stroke bounding box pads and clamps to the display", () => {
  const points = [
    { x: 100, y: 100 },
    { x: 300, y: 120 },
    { x: 280, y: 240 },
    { x: 110, y: 220 },
  ];
  const box = strokeBoundingBox(points, 1512, 982);
  assert.ok(box.x < 100 && box.y < 100, "padding extends beyond the stroke");
  assert.ok(box.x + box.width > 300 && box.y + box.height > 240);
  assert.ok(box.x >= 0 && box.y >= 0);
  // A tiny accidental flick is rejected.
  assert.equal(strokeBoundingBox([{ x: 5, y: 5 }, { x: 6, y: 7 }], 1512, 982), null);
});

test("sanitizes whisper artifacts", () => {
  assert.equal(sanitizeTranscript("[BLANK_AUDIO]"), "");
  assert.equal(sanitizeTranscript(" (wind blowing)  hello there "), "hello there");
  assert.equal(sanitizeTranscript("replicate this search bar"), "replicate this search bar");
});

test("system prompt carries the two tag protocols", () => {
  const p = buildCompanionSystemPrompt({ screens: 2 });
  assert.ok(p.includes("[POINT:x,y:label]"));
  assert.ok(p.includes("[TASK:"));
  assert.ok(p.includes("primary focus"));
});
