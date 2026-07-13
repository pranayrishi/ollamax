// Local-only POINT directive parser shared by the desktop renderer and main
// process.  A directive is deliberately narrow:
//
//   [POINT:0.35,0.60:Search field:screen0]
//
// Coordinates are normalized to the selected display (0..1 inclusive), the
// optional screen index is zero-based, and the short ASCII label is rendered
// with textContent only.  This module never reads the screen or controls the
// pointer; it merely validates data for a click-through visual cue.
(function exposePointDirectives(root, factory) {
  const api = factory();
  if (typeof module === "object" && module.exports) module.exports = api;
  if (root) root.OllamaxPointDirectives = api;
})(typeof globalThis === "object" ? globalThis : this, function makePointDirectives() {
  "use strict";

  const MAX_POINT_LABEL_LENGTH = 80;
  const MAX_POINT_DIRECTIVE_LENGTH = 180;
  const MAX_POINT_DIRECTIVES_PER_RESPONSE = 3;
  const MAX_SCREEN_INDEX = 15;
  const DIRECTIVE_PREFIX = "[POINT:";
  // Decimal-only coordinates keep the wire format unambiguous and avoid
  // accepting exponent, sign, or special-number spellings from model output.
  const NORMALIZED_DECIMAL = "(?:0(?:\\.\\d{1,6})?|1(?:\\.0{1,6})?)";
  const DIRECTIVE_PATTERN = new RegExp(
    "^\\[POINT:(" + NORMALIZED_DECIMAL + "),(" + NORMALIZED_DECIMAL + "):([^:\\[\\]\\r\\n]{1," +
      MAX_POINT_LABEL_LENGTH + "})(?::screen([0-9]|1[0-5]))?\\]$"
  );
  const LABEL_PATTERN = /^[A-Za-z0-9][A-Za-z0-9 .,'()/_+\-]*$/;
  const hasOwn = (value, key) => Object.prototype.hasOwnProperty.call(value, key);

  function isNormalizedCoordinate(value) {
    return typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= 1;
  }

  function isSafeLabel(value) {
    return (
      typeof value === "string" &&
      value.length > 0 &&
      value.length <= MAX_POINT_LABEL_LENGTH &&
      value === value.trim() &&
      LABEL_PATTERN.test(value)
    );
  }

  // Revalidates an IPC payload.  Do not trust the renderer merely because the
  // parser originated there: a page can call its exposed bridge directly.
  function validatePointDirective(value) {
    if (!value || typeof value !== "object" || Array.isArray(value)) return null;
    if (!hasOwn(value, "x") || !hasOwn(value, "y") || !hasOwn(value, "label")) return null;
    if (!isNormalizedCoordinate(value.x) || !isNormalizedCoordinate(value.y) || !isSafeLabel(value.label)) {
      return null;
    }

    const rawScreenIndex = hasOwn(value, "screenIndex") ? value.screenIndex : null;
    const screenIndex = rawScreenIndex == null ? null : rawScreenIndex;
    if (
      screenIndex !== null &&
      (!Number.isInteger(screenIndex) || screenIndex < 0 || screenIndex > MAX_SCREEN_INDEX)
    ) {
      return null;
    }

    return Object.freeze({
      x: value.x,
      y: value.y,
      label: value.label,
      screenIndex,
    });
  }

  function parsePointDirective(candidate) {
    if (typeof candidate !== "string" || candidate.length > MAX_POINT_DIRECTIVE_LENGTH) return null;
    const match = DIRECTIVE_PATTERN.exec(candidate);
    if (!match) return null;
    return validatePointDirective({
      x: Number(match[1]),
      y: Number(match[2]),
      label: match[3],
      screenIndex: match[4] == null ? null : Number(match[4]),
    });
  }

  // Only completed directive-shaped spans are removed.  Invalid completed
  // directives are also stripped so malformed model control text can neither
  // reach visible chat/speech output nor become a cue.  The original prose and
  // code whitespace are otherwise preserved exactly.
  function parsePointDirectives(text) {
    const source = typeof text === "string" ? text : String(text == null ? "" : text);
    const directives = [];
    const parts = [];
    let offset = 0;

    while (offset < source.length) {
      const start = source.indexOf(DIRECTIVE_PREFIX, offset);
      if (start === -1) {
        parts.push(source.slice(offset));
        break;
      }
      const end = source.indexOf("]", start + DIRECTIVE_PREFIX.length);
      if (end === -1) {
        parts.push(source.slice(offset));
        break;
      }

      parts.push(source.slice(offset, start));
      const candidate = source.slice(start, end + 1);
      const directive = parsePointDirective(candidate);
      if (directive && directives.length < MAX_POINT_DIRECTIVES_PER_RESPONSE) {
        directives.push(directive);
      }
      // Strip the entire completed candidate even if it exceeds the parser's
      // hard length limit. This bounds cue data while keeping control markup
      // out of the finalized response.
      offset = end + 1;
    }

    return Object.freeze({ text: parts.join(""), directives: Object.freeze(directives) });
  }

  return Object.freeze({
    DIRECTIVE_PREFIX,
    MAX_POINT_DIRECTIVE_LENGTH,
    MAX_POINT_DIRECTIVES_PER_RESPONSE,
    MAX_POINT_LABEL_LENGTH,
    MAX_SCREEN_INDEX,
    parsePointDirective,
    parsePointDirectives,
    validatePointDirective,
  });
});
