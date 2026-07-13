'use strict';

/**
 * Small, dependency-free geometry helpers for a screen-selection overlay.
 *
 * Coordinates passed to the lasso and bounds functions are display-independent
 * pixels (DIPs), matching Electron's display bounds. Screenshot coordinates are
 * physical image pixels. Keeping those spaces explicit prevents a Retina-scale
 * selection from being cropped at the wrong location.
 */

const DEFAULT_CROP_LIMITS = Object.freeze({
  maxWidth: 1600,
  maxHeight: 1600,
  maxPixels: 2_000_000,
});

const DEFAULT_NORMALIZE_OPTIONS = Object.freeze({
  minDistance: 0.5,
  maxSamples: 512,
});

// Electron IPC data is normally plain JSON, but this guard sits before any
// geometry loop so an unexpectedly large or malformed renderer payload cannot
// turn a lasso completion into unbounded main-process work.
const MAX_LASSO_INPUT_SAMPLES = 2_048;

function assertBoundedLassoInput(samples, maximum = MAX_LASSO_INPUT_SAMPLES) {
  if (!Array.isArray(samples)) {
    throw new TypeError("lasso samples must be an array");
  }
  if (!Number.isSafeInteger(maximum) || maximum < 1) {
    throw new TypeError("maximum lasso samples must be a positive integer");
  }
  if (samples.length > maximum) {
    throw new RangeError(`lasso selection contains too many points (maximum ${maximum})`);
  }
  return samples;
}

/**
 * Returns a cleaned, bounded copy of lasso points.
 *
 * Invalid samples are ignored. Nearby consecutive samples are collapsed using
 * `minDistance`, then a uniform resample preserves the first and last point
 * when `maxSamples` is smaller than the input.
 *
 * @param {Iterable<{x: number, y: number}> | null | undefined} samples
 * @param {{minDistance?: number, maxSamples?: number}} [options]
 * @returns {{x: number, y: number}[]}
 */
function normalizeLassoSamples(samples, options = {}) {
  const resolvedOptions = requirePlainObject(options, 'options');
  const minDistance = readNonNegativeNumber(
    resolvedOptions.minDistance ?? DEFAULT_NORMALIZE_OPTIONS.minDistance,
    'options.minDistance',
  );
  const maxSamples = readPositiveInteger(
    resolvedOptions.maxSamples ?? DEFAULT_NORMALIZE_OPTIONS.maxSamples,
    'options.maxSamples',
  );

  if (samples == null) {
    return [];
  }
  if (typeof samples[Symbol.iterator] !== 'function') {
    throw new TypeError('samples must be iterable');
  }

  const minDistanceSquared = minDistance * minDistance;
  const normalized = [];

  for (const sample of samples) {
    const point = readPoint(sample);
    if (!point) {
      continue;
    }

    const previous = normalized[normalized.length - 1];
    if (!previous || squaredDistance(previous, point) > minDistanceSquared) {
      normalized.push(point);
    }
  }

  if (normalized.length <= maxSamples) {
    return normalized;
  }

  if (maxSamples === 1) {
    return [normalized[0]];
  }

  const result = [];
  const lastIndex = normalized.length - 1;
  for (let index = 0; index < maxSamples; index += 1) {
    const sourceIndex = Math.round((index * lastIndex) / (maxSamples - 1));
    result.push(normalized[sourceIndex]);
  }
  return result;
}

/**
 * Builds a padded, minimum-size selection rectangle in display DIP space.
 *
 * The raw lasso bounds must touch the display. The result is then expanded to
 * the requested minimum content size, padded, and shifted (or reduced only if
 * necessary) to stay entirely inside the display. `null` means the lasso has
 * no usable point or does not touch that display.
 *
 * @param {Iterable<{x: number, y: number}> | null | undefined} samples
 * @param {{x: number, y: number, width: number, height: number}} displayBounds
 * @param {{padding?: number, minWidth?: number, minHeight?: number}} [options]
 * @returns {{x: number, y: number, width: number, height: number} | null}
 */
function buildPaddedSelectionBounds(samples, displayBounds, options = {}) {
  const display = readBounds(displayBounds, 'displayBounds');
  const resolvedOptions = requirePlainObject(options, 'options');
  const padding = readNonNegativeNumber(resolvedOptions.padding ?? 12, 'options.padding');
  const minWidth = readNonNegativeNumber(resolvedOptions.minWidth ?? 24, 'options.minWidth');
  const minHeight = readNonNegativeNumber(resolvedOptions.minHeight ?? 24, 'options.minHeight');
  const points = collectFinitePoints(samples);

  if (points.length === 0) {
    return null;
  }

  const rawBounds = boundsFromPoints(points);
  if (!rawBounds || !rectanglesTouchOrIntersect(rawBounds, display)) {
    return null;
  }

  const width = checkedFiniteSum(Math.max(rawBounds.width, minWidth), padding * 2, 'selection width');
  const height = checkedFiniteSum(Math.max(rawBounds.height, minHeight), padding * 2, 'selection height');
  const centerX = rawBounds.x + rawBounds.width / 2;
  const centerY = rawBounds.y + rawBounds.height / 2;

  if (!Number.isFinite(centerX) || !Number.isFinite(centerY)) {
    return null;
  }

  const requestedBounds = {
    x: centerX - width / 2,
    y: centerY - height / 2,
    width,
    height,
  };
  if (!Number.isFinite(requestedBounds.x) || !Number.isFinite(requestedBounds.y)) {
    return null;
  }

  return fitRectWithinBounds(requestedBounds, display);
}

/**
 * Maps a display-DIP rectangle to a physical screenshot pixel rectangle.
 *
 * `display.scaleFactor` is used to derive an image size when no screenshot
 * dimensions are supplied. If screenshot dimensions are supplied, their real
 * dimensions are authoritative; this handles captures whose horizontal and
 * vertical image scales differ slightly from Electron's reported scale factor.
 *
 * A zero-area point that touches a display maps to one screenshot pixel so a
 * click-like selection remains actionable. Returns `null` for no overlap.
 *
 * @param {{x: number, y: number, width: number, height: number}} selectionDip
 * @param {{x: number, y: number, width: number, height: number, scaleFactor?: number}} display
 * @param {{width: number, height: number}} [screenshotPixels]
 * @returns {{x: number, y: number, width: number, height: number, scaleX: number, scaleY: number, sourceWidth: number, sourceHeight: number} | null}
 */
function mapDipRectToScreenshotPixels(selectionDip, display, screenshotPixels) {
  const displayBounds = readBounds(display, 'display');
  const selection = readNonNegativeRect(selectionDip, 'selectionDip');
  const source = resolveScreenshotSize(display, displayBounds, screenshotPixels);
  const clipped = intersectRectsAllowingEdges(selection, displayBounds);

  if (!clipped || !hasActionableDipIntersection(selection, clipped)) {
    return null;
  }

  const scaleX = source.width / displayBounds.width;
  const scaleY = source.height / displayBounds.height;
  const mappedX = mapDipAxisToPixels(
    clipped.x,
    clipped.width,
    displayBounds.x,
    scaleX,
    source.width,
  );
  const mappedY = mapDipAxisToPixels(
    clipped.y,
    clipped.height,
    displayBounds.y,
    scaleY,
    source.height,
  );

  return {
    x: mappedX.start,
    y: mappedY.start,
    width: mappedX.end - mappedX.start,
    height: mappedY.end - mappedY.start,
    scaleX,
    scaleY,
    sourceWidth: source.width,
    sourceHeight: source.height,
  };
}

/**
 * Produces a crop-and-resize plan without touching image data.
 *
 * The crop is rounded outward and clipped to the screenshot. Output limits
 * preserve aspect ratio whenever possible, never upscale, and always satisfy
 * width, height, and pixel-count caps. `null` means there is no positive-area
 * crop after clipping.
 *
 * @param {{x: number, y: number, width: number, height: number}} pixelRect
 * @param {{width: number, height: number}} sourcePixels
 * @param {{maxWidth?: number, maxHeight?: number, maxPixels?: number}} [options]
 * @returns {{sourceRect: {x: number, y: number, width: number, height: number}, outputSize: {width: number, height: number}, scaleX: number, scaleY: number, downscaled: boolean} | null}
 */
function planCappedCrop(pixelRect, sourcePixels, options = {}) {
  const source = readPixelSize(sourcePixels, 'sourcePixels');
  const requested = readNonNegativeRect(pixelRect, 'pixelRect');
  const limits = readCropLimits(options);
  const clipped = intersectRectsAllowingEdges(requested, {
    x: 0,
    y: 0,
    width: source.width,
    height: source.height,
  });

  if (!clipped) {
    return null;
  }

  const sourceRect = roundOutwardPixelRect(clipped, source);
  if (!sourceRect) {
    return null;
  }

  const outputSize = fitOutputSize(sourceRect.width, sourceRect.height, limits);
  return {
    sourceRect,
    outputSize,
    scaleX: outputSize.width / sourceRect.width,
    scaleY: outputSize.height / sourceRect.height,
    downscaled: outputSize.width !== sourceRect.width || outputSize.height !== sourceRect.height,
  };
}

function collectFinitePoints(samples) {
  if (samples == null) {
    return [];
  }
  if (typeof samples[Symbol.iterator] !== 'function') {
    throw new TypeError('samples must be iterable');
  }

  const points = [];
  for (const sample of samples) {
    const point = readPoint(sample);
    if (point) {
      points.push(point);
    }
  }
  return points;
}

function readPoint(value) {
  if (!value || typeof value !== 'object') {
    return null;
  }
  if (!Number.isFinite(value.x) || !Number.isFinite(value.y)) {
    return null;
  }
  return { x: value.x, y: value.y };
}

function boundsFromPoints(points) {
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;

  for (const point of points) {
    minX = Math.min(minX, point.x);
    minY = Math.min(minY, point.y);
    maxX = Math.max(maxX, point.x);
    maxY = Math.max(maxY, point.y);
  }

  const width = maxX - minX;
  const height = maxY - minY;
  if (!Number.isFinite(width) || !Number.isFinite(height)) {
    return null;
  }
  return { x: minX, y: minY, width, height };
}

function fitRectWithinBounds(rect, bounds) {
  const width = Math.min(rect.width, bounds.width);
  const height = Math.min(rect.height, bounds.height);
  const maxX = bounds.x + bounds.width - width;
  const maxY = bounds.y + bounds.height - height;

  return {
    x: clamp(rect.x, bounds.x, maxX),
    y: clamp(rect.y, bounds.y, maxY),
    width,
    height,
  };
}

function rectanglesTouchOrIntersect(first, second) {
  const firstRight = rightEdge(first);
  const firstBottom = bottomEdge(first);
  const secondRight = rightEdge(second);
  const secondBottom = bottomEdge(second);
  return first.x <= secondRight
    && firstRight >= second.x
    && first.y <= secondBottom
    && firstBottom >= second.y;
}

function intersectRectsAllowingEdges(first, second) {
  const x = Math.max(first.x, second.x);
  const y = Math.max(first.y, second.y);
  const right = Math.min(rightEdge(first), rightEdge(second));
  const bottom = Math.min(bottomEdge(first), bottomEdge(second));

  if (right < x || bottom < y) {
    return null;
  }
  return { x, y, width: right - x, height: bottom - y };
}

function hasActionableDipIntersection(selection, clipped) {
  // A deliberate point/line selection can be promoted to a one-pixel crop.
  // A positive-area rectangle that merely grazes an outer display edge cannot.
  return !(selection.width > 0 && clipped.width === 0)
    && !(selection.height > 0 && clipped.height === 0);
}

function resolveScreenshotSize(display, displayBounds, screenshotPixels) {
  const scaleFactor = readPositiveNumber(display.scaleFactor ?? 1, 'display.scaleFactor');
  if (screenshotPixels !== undefined) {
    return readPixelSize(screenshotPixels, 'screenshotPixels');
  }

  return {
    width: roundedPositiveInteger(displayBounds.width * scaleFactor, 'derived screenshot width'),
    height: roundedPositiveInteger(displayBounds.height * scaleFactor, 'derived screenshot height'),
  };
}

function mapDipAxisToPixels(value, length, displayOrigin, scale, sourceLength) {
  const relativeStart = (value - displayOrigin) * scale;
  const relativeEnd = relativeStart + length * scale;
  let start = clamp(Math.floor(relativeStart), 0, sourceLength);
  let end = clamp(Math.ceil(relativeEnd), 0, sourceLength);

  if (end <= start) {
    // This happens for a zero-area selection, including one at the far edge.
    start = clamp(Math.floor(relativeStart), 0, sourceLength - 1);
    end = start + 1;
  }
  return { start, end };
}

function roundOutwardPixelRect(rect, source) {
  const x = clamp(Math.floor(rect.x), 0, source.width);
  const y = clamp(Math.floor(rect.y), 0, source.height);
  const right = clamp(Math.ceil(rightEdge(rect)), 0, source.width);
  const bottom = clamp(Math.ceil(bottomEdge(rect)), 0, source.height);

  if (right <= x || bottom <= y) {
    return null;
  }
  return { x, y, width: right - x, height: bottom - y };
}

function readCropLimits(options) {
  const resolvedOptions = requirePlainObject(options, 'options');
  return {
    maxWidth: readPixelLimit(
      resolvedOptions.maxWidth ?? DEFAULT_CROP_LIMITS.maxWidth,
      'options.maxWidth',
    ),
    maxHeight: readPixelLimit(
      resolvedOptions.maxHeight ?? DEFAULT_CROP_LIMITS.maxHeight,
      'options.maxHeight',
    ),
    maxPixels: readPixelLimit(
      resolvedOptions.maxPixels ?? DEFAULT_CROP_LIMITS.maxPixels,
      'options.maxPixels',
    ),
  };
}

function fitOutputSize(width, height, limits) {
  const sourcePixels = width * height;
  const pixelScale = Number.isFinite(limits.maxPixels)
    ? Math.sqrt(limits.maxPixels / sourcePixels)
    : Infinity;
  const scale = Math.min(
    1,
    limits.maxWidth / width,
    limits.maxHeight / height,
    pixelScale,
  );

  let outputWidth = Math.max(1, Math.floor(width * scale));
  let outputHeight = Math.max(1, Math.floor(height * scale));
  outputWidth = Math.min(outputWidth, limits.maxWidth);
  outputHeight = Math.min(outputHeight, limits.maxHeight);

  // Rounding a very thin crop to at least one pixel can violate maxPixels.
  // Correct it directly instead of using a decrement loop that could be large.
  if (Number.isFinite(limits.maxPixels) && outputWidth * outputHeight > limits.maxPixels) {
    if (outputWidth >= outputHeight) {
      outputWidth = Math.max(1, Math.floor(limits.maxPixels / outputHeight));
      if (outputWidth * outputHeight > limits.maxPixels) {
        outputHeight = Math.max(1, Math.floor(limits.maxPixels / outputWidth));
      }
    } else {
      outputHeight = Math.max(1, Math.floor(limits.maxPixels / outputWidth));
      if (outputWidth * outputHeight > limits.maxPixels) {
        outputWidth = Math.max(1, Math.floor(limits.maxPixels / outputHeight));
      }
    }
  }

  return { width: outputWidth, height: outputHeight };
}

function readBounds(value, name) {
  const rect = readNonNegativeRect(value, name);
  if (rect.width <= 0 || rect.height <= 0) {
    throw new RangeError(`${name}.width and ${name}.height must be greater than zero`);
  }
  return rect;
}

function readNonNegativeRect(value, name) {
  if (!value || typeof value !== 'object') {
    throw new TypeError(`${name} must be an object with x, y, width, and height`);
  }
  const x = readFiniteNumber(value.x, `${name}.x`);
  const y = readFiniteNumber(value.y, `${name}.y`);
  const width = readNonNegativeNumber(value.width, `${name}.width`);
  const height = readNonNegativeNumber(value.height, `${name}.height`);
  const right = x + width;
  const bottom = y + height;
  if (!Number.isFinite(right) || !Number.isFinite(bottom)) {
    throw new RangeError(`${name} extends beyond the supported coordinate range`);
  }
  return { x, y, width, height };
}

function readPixelSize(value, name) {
  if (!value || typeof value !== 'object') {
    throw new TypeError(`${name} must be an object with width and height`);
  }
  return {
    width: readPositiveInteger(value.width, `${name}.width`),
    height: readPositiveInteger(value.height, `${name}.height`),
  };
}

function readPixelLimit(value, name) {
  if (value === Infinity) {
    return Infinity;
  }
  return readPositiveInteger(value, name);
}

function roundedPositiveInteger(value, name) {
  if (!Number.isFinite(value) || value <= 0) {
    throw new RangeError(`${name} must resolve to a positive finite number`);
  }
  return Math.max(1, Math.round(value));
}

function readPositiveInteger(value, name) {
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new RangeError(`${name} must be a positive safe integer`);
  }
  return value;
}

function readPositiveNumber(value, name) {
  if (!Number.isFinite(value) || value <= 0) {
    throw new RangeError(`${name} must be a positive finite number`);
  }
  return value;
}

function readNonNegativeNumber(value, name) {
  if (!Number.isFinite(value) || value < 0) {
    throw new RangeError(`${name} must be a non-negative finite number`);
  }
  return value;
}

function readFiniteNumber(value, name) {
  if (!Number.isFinite(value)) {
    throw new RangeError(`${name} must be a finite number`);
  }
  return value;
}

function requirePlainObject(value, name) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new TypeError(`${name} must be an object`);
  }
  return value;
}

function squaredDistance(first, second) {
  const deltaX = second.x - first.x;
  const deltaY = second.y - first.y;
  return deltaX * deltaX + deltaY * deltaY;
}

function rightEdge(rect) {
  return rect.x + rect.width;
}

function bottomEdge(rect) {
  return rect.y + rect.height;
}

function checkedFiniteSum(first, second, name) {
  const result = first + second;
  if (!Number.isFinite(result)) {
    throw new RangeError(`${name} is too large`);
  }
  return result;
}

function clamp(value, minimum, maximum) {
  return Math.min(Math.max(value, minimum), maximum);
}

module.exports = {
  MAX_LASSO_INPUT_SAMPLES,
  assertBoundedLassoInput,
  DEFAULT_CROP_LIMITS,
  DEFAULT_NORMALIZE_OPTIONS,
  normalizeLassoSamples,
  buildPaddedSelectionBounds,
  mapDipRectToScreenshotPixels,
  planCappedCrop,
};
