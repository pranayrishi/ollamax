"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const {
  MAX_POINT_DIRECTIVES_PER_RESPONSE,
  MAX_POINT_LABEL_LENGTH,
  parsePointDirective,
  parsePointDirectives,
  validatePointDirective,
} = require("../renderer/point-directives");

test("parses a bounded normalized POINT directive and removes it from final text", () => {
  const parsed = parsePointDirectives(
    "I found the field.\n[POINT:0.35,0.60:Search field:screen0]\nYou can reuse it."
  );

  assert.equal(parsed.text, "I found the field.\n\nYou can reuse it.");
  assert.deepEqual(parsed.directives, [
    { x: 0.35, y: 0.6, label: "Search field", screenIndex: 0 },
  ]);
  assert.ok(Object.isFrozen(parsed));
  assert.ok(Object.isFrozen(parsed.directives));
});

test("POINT syntax is strict and invalid completed directives are stripped without becoming cues", () => {
  const invalid = [
    "[POINT:-0.1,0.5:Search]",
    "[POINT:1.000001,0.5:Search]",
    "[POINT:1e-1,0.5:Search]",
    "[POINT:0.5,.5:Search]",
    "[POINT:0.5,0.5: leading space]",
    "[POINT:0.5,0.5:Search:screen16]",
    "[POINT:0.5,0.5:Search#1]",
  ];

  for (const directive of invalid) {
    const parsed = parsePointDirectives(`Before ${directive} after`);
    assert.equal(parsed.text, "Before  after", directive);
    assert.deepEqual(parsed.directives, [], directive);
  }
});

test("POINT parser preserves surrounding prose and code whitespace while stripping only directives", () => {
  const parsed = parsePointDirectives(
    "const value = 1;\n[POINT:0,1:Bottom result]\n  return value;"
  );
  assert.equal(parsed.text, "const value = 1;\n\n  return value;");
  assert.deepEqual(parsed.directives, [
    { x: 0, y: 1, label: "Bottom result", screenIndex: null },
  ]);
});

test("POINT parser caps cues per completed response but removes every directive", () => {
  const text = ["A", "B", "C", "D"]
    .map((label, index) => `[POINT:0.${index + 1},0.5:${label}]`)
    .join("");
  const parsed = parsePointDirectives(text);

  assert.equal(parsed.text, "");
  assert.equal(parsed.directives.length, MAX_POINT_DIRECTIVES_PER_RESPONSE);
  assert.deepEqual(parsed.directives.map((directive) => directive.label), ["A", "B", "C"]);
});

test("IPC validator accepts only finite normalized values and a capped safe label", () => {
  assert.deepEqual(
    validatePointDirective({ x: 0, y: 1, label: "Search field", screenIndex: 0 }),
    { x: 0, y: 1, label: "Search field", screenIndex: 0 }
  );
  assert.equal(validatePointDirective({ x: NaN, y: 0.5, label: "Search" }), null);
  assert.equal(validatePointDirective({ x: 0.5, y: 0.5, label: "Search", screenIndex: 16 }), null);
  assert.equal(
    validatePointDirective({ x: 0.5, y: 0.5, label: "x".repeat(MAX_POINT_LABEL_LENGTH + 1) }),
    null
  );
  assert.equal(validatePointDirective({ x: 0.5, y: 0.5, label: "Search\nfield" }), null);
});

test("single directive parser rejects overlong candidates before validation", () => {
  const tooLong = `[POINT:0.5,0.5:${"x".repeat(MAX_POINT_LABEL_LENGTH + 1)}]`;
  assert.equal(parsePointDirective(tooLong), null);
});

test("desktop finalizer strips directives before handing bounded cues to the native bridge", () => {
  const sent = [];
  const window = {
    OllamaxPointDirectives: require("../renderer/point-directives"),
    forgeNative: {
      buddy: {
        point(directive) {
          sent.push(directive);
          return Promise.resolve({ ok: true });
        },
      },
    },
  };
  const source = fs.readFileSync(path.join(__dirname, "..", "renderer", "desktop-points.js"), "utf8");
  vm.runInNewContext(source, { Promise, window });

  const cleaned = window.__ollamaxFinalizeAssistantResponse(
    "Use this. [POINT:0.25,0.75:Search field:screen0]"
  );
  assert.equal(cleaned, "Use this. ");
  assert.deepEqual(sent, [{ x: 0.25, y: 0.75, label: "Search field", screenIndex: 0 }]);
});
