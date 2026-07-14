// Companion protocol — the pure logic of the voice companion, kept free of
// Electron imports so it can be unit-tested with plain `node --test`.
//
// The companion speaks a small tag protocol with the local vision model:
//   - "[POINT:x,y:label]" / "[POINT:none]" (optionally ":screenN") at the end
//     of a reply makes the overlay cursor fly to a UI element. Coordinates are
//     in the SCREENSHOT's pixel space (top-left origin).
//   - "[TASK: one-line coding task]" after the POINT tag hands work off to the
//     main Ollamax window (e.g. "replicate this search bar in my project").
//     The user always reviews before it is sent — never auto-submitted.
"use strict";

// [POINT:none] | [POINT:x,y] | [POINT:x,y:label] | [POINT:x,y:label:screenN]
const POINT_RE =
  /\[POINT:(?:none|(\d+)\s*,\s*(\d+)(?::([^\]:][^\]:]*?))?(?::screen(\d+))?)\]\s*$/i;
// [TASK: build a matching search bar component in src/components]
const TASK_RE = /\[TASK:\s*([^\]]{1,400}?)\s*\]\s*$/i;

/** Remove inline <think>…</think> blocks some local reasoning models emit. */
function stripThinkBlocks(text) {
  return String(text || "").replace(/<think>[\s\S]*?<\/think>/gi, "");
}

/**
 * Parse a full companion reply into what should be SPOKEN, an optional
 * pointing target, and an optional coding-task handoff.
 *
 * Tag order at the end of a reply is [POINT:...][TASK:...]; both are optional
 * and each is parsed only at the end so coordinates quoted mid-sentence are
 * left alone.
 */
function parseCompanionReply(rawText) {
  let text = stripThinkBlocks(rawText).trim();

  let task = null;
  const taskMatch = text.match(TASK_RE);
  if (taskMatch) {
    task = taskMatch[1].trim() || null;
    text = text.slice(0, taskMatch.index).trimEnd();
  }

  let point = null;
  const pointMatch = text.match(POINT_RE);
  if (pointMatch) {
    const [, x, y, label, screen] = pointMatch;
    if (x !== undefined && y !== undefined) {
      point = {
        x: Number(x),
        y: Number(y),
        label: (label || "").trim() || null,
        screenNumber: screen !== undefined ? Number(screen) : null,
      };
    }
    text = text.slice(0, pointMatch.index).trimEnd();
  }

  return { spokenText: text.trim(), point, task };
}

/**
 * Map a POINT coordinate from screenshot pixel space into display-local DIP
 * coordinates for the overlay window covering that display. Electron display
 * bounds are already top-left-origin, so no y-flip is needed (unlike AppKit).
 */
function scalePointToDisplay(point, imageWidthPx, imageHeightPx, displayBounds) {
  if (!point || !imageWidthPx || !imageHeightPx) return null;
  const clampedX = Math.max(0, Math.min(point.x, imageWidthPx));
  const clampedY = Math.max(0, Math.min(point.y, imageHeightPx));
  return {
    x: (clampedX * displayBounds.width) / imageWidthPx,
    y: (clampedY * displayBounds.height) / imageHeightPx,
  };
}

/**
 * Bounding box of a freehand "circle around it" stroke, padded so the crop
 * keeps a little surrounding context, clamped to the display.
 * Points and the returned rect are display-local DIP coordinates.
 */
function strokeBoundingBox(points, displayWidth, displayHeight) {
  if (!Array.isArray(points) || points.length < 2) return null;
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const p of points) {
    if (!p || typeof p.x !== "number" || typeof p.y !== "number") continue;
    minX = Math.min(minX, p.x);
    minY = Math.min(minY, p.y);
    maxX = Math.max(maxX, p.x);
    maxY = Math.max(maxY, p.y);
  }
  if (!isFinite(minX) || maxX - minX < 4 || maxY - minY < 4) return null;
  // 8% of the region (min 12 DIP) of breathing room on every side.
  const padX = Math.max(12, (maxX - minX) * 0.08);
  const padY = Math.max(12, (maxY - minY) * 0.08);
  const x = Math.max(0, Math.floor(minX - padX));
  const y = Math.max(0, Math.floor(minY - padY));
  return {
    x,
    y,
    width: Math.min(displayWidth - x, Math.ceil(maxX - minX + 2 * padX)),
    height: Math.min(displayHeight - y, Math.ceil(maxY - minY + 2 * padY)),
  };
}

/**
 * Whisper occasionally transcribes silence as bracketed stage directions
 * ("[BLANK_AUDIO]", "(wind blowing)"). Strip them; an empty result means
 * "no speech" and the turn should be dropped instead of sent to the model.
 */
function sanitizeTranscript(raw) {
  return String(raw || "")
    .replace(/\[[^\]]*\]/g, " ")
    .replace(/\([^)]*\)/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

/**
 * The companion persona. Written for text-to-speech output (short, spoken
 * register) with the POINT protocol for element pointing and the TASK
 * protocol for handing real coding work to the Ollamax agent. Runs entirely
 * on the local vision model — never assume cloud capabilities.
 */
function buildCompanionSystemPrompt({ screens = 1 } = {}) {
  const multi =
    screens > 1
      ? `you may receive ${screens} screen images. the one labeled "primary focus" is where the cursor is — prioritize it, but reference the others if relevant. `
      : "";
  return `you're the ollamax companion, a friendly always-on helper that lives beside the user's cursor. the user just spoke to you via push-to-talk and you can see their screen. your reply will be spoken aloud via text-to-speech, so write the way you'd actually talk. this is an ongoing conversation — you remember what they said before.

rules:
- default to one or two sentences. be direct and dense. if the user asks you to go deeper or elaborate, give a thorough explanation with no length limit.
- all lowercase, casual, warm. no emojis.
- write for the ear, not the eye. short sentences. no lists, bullet points, markdown, or formatting — just natural speech.
- don't use abbreviations or symbols that sound weird read aloud. write "for example" not "e.g.", spell out small numbers.
- if the user's question relates to what's on their screen, reference specific things you see. ${multi}
- if the screenshot isn't relevant to the question, just answer the question directly.
- you can help with anything — coding, writing, general knowledge, brainstorming.
- never say "simply" or "just".
- don't read out code verbatim. describe what the code does or what needs to change, conversationally.
- don't end with dead-end yes/no questions. when it fits, end by planting a seed — something more ambitious they could try next.

element pointing:
you have a small cursor that can fly to and point at things on screen. use it whenever pointing would genuinely help — finding a menu, a button, a field. err on the side of pointing; it makes your help concrete. don't point for general-knowledge answers or at something the user is obviously already looking at.

when you point, append a coordinate tag at the very end of your response, after your spoken text. the screenshot images are labeled with their pixel dimensions — use that coordinate space, origin at the top-left, x rightward, y downward.

format: [POINT:x,y:label] with integer pixel coordinates and a one-to-three word label like "search bar" or "save button". if the element is on a different screen than the cursor, append :screenN using the screen number from the image label. if pointing wouldn't help, append [POINT:none].

task handoff:
the user has a full local coding agent (ollamax) open. when they ask you to BUILD, REPLICATE, or CHANGE code — for example "replicate this search bar in my project" or "add a button like that one" — do two things: first, briefly say in your spoken reply what you'll set up; second, append a task tag at the very end, after the point tag, of the form [TASK: one clear sentence describing the coding task, mentioning the circled or pointed-at element]. the task goes to the coding agent with the relevant screenshot attached, and the user reviews it before it runs. only emit [TASK: ...] when the user actually asked for code or a build — never for questions or explanations.

examples:
- "how do i commit in this app" → "see that source control menu up top? click it and hit commit. [POINT:285,11:source control]"
- "what is html" → "html is the skeleton of every web page — the structure the styling hangs off. [POINT:none]"
- "replicate this search bar in my app" (user circled a search bar) → "nice pick — i'll have the agent build a matching search bar with that rounded style and the icon on the left. [POINT:none][TASK: replicate the circled search bar (rounded input with left magnifier icon and placeholder text) as a reusable component in the user's current project]"`;
}

/**
 * Extra system context describing the circled region for spatial-context
 * turns. Sent as part of the user message (not the persona) so it applies to
 * exactly one turn.
 */
function describeCircledRegion(regionIndexLabel, bbox) {
  return `note: the user circled a region of the screen with the mouse. the image labeled "${regionIndexLabel}" is a crop of exactly that region (bounding box x=${bbox.x}, y=${bbox.y}, width=${bbox.width}, height=${bbox.height} in screen coordinates). their spoken instruction refers to that region.`;
}

module.exports = {
  parseCompanionReply,
  stripThinkBlocks,
  scalePointToDisplay,
  strokeBoundingBox,
  sanitizeTranscript,
  buildCompanionSystemPrompt,
  describeCircledRegion,
};
