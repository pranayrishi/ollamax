'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');

const {
  MAX_LASSO_INPUT_SAMPLES,
  assertBoundedLassoInput,
  buildPaddedSelectionBounds,
  mapDipRectToScreenshotPixels,
  normalizeLassoSamples,
  planCappedCrop,
} = require('./spatial-selection');

test('assertBoundedLassoInput rejects non-array and oversized IPC payloads before geometry work', () => {
  const allowed = [{ x: 1, y: 2 }];
  assert.strictEqual(assertBoundedLassoInput(allowed), allowed);
  assert.throws(() => assertBoundedLassoInput(new Set(allowed)), /array/);
  assert.throws(
    () => assertBoundedLassoInput(Array.from({ length: MAX_LASSO_INPUT_SAMPLES + 1 }, () => ({ x: 0, y: 0 }))),
    /too many points/,
  );
});

test('normalizeLassoSamples ignores invalid input, collapses nearby samples, and leaves input untouched', () => {
  const samples = [
    { x: 0, y: 0 },
    { x: 0.2, y: 0.2 },
    null,
    { x: Number.NaN, y: 3 },
    { x: 1, y: 0 },
    { x: 1, y: 0 },
    { x: 3, y: 4 },
    { x: Infinity, y: 5 },
  ];

  const result = normalizeLassoSamples(samples, { minDistance: 0.5, maxSamples: 10 });

  assert.deepEqual(result, [
    { x: 0, y: 0 },
    { x: 1, y: 0 },
    { x: 3, y: 4 },
  ]);
  assert.notStrictEqual(result[0], samples[0]);
  assert.deepEqual(samples[0], { x: 0, y: 0 });
});

test('normalizeLassoSamples uniformly caps a long lasso while preserving endpoints', () => {
  const samples = Array.from({ length: 10 }, (_, x) => ({ x, y: 0 }));

  assert.deepEqual(normalizeLassoSamples(samples, { minDistance: 0, maxSamples: 4 }), [
    { x: 0, y: 0 },
    { x: 3, y: 0 },
    { x: 6, y: 0 },
    { x: 9, y: 0 },
  ]);
  assert.deepEqual(normalizeLassoSamples(samples, { minDistance: 0, maxSamples: 1 }), [
    { x: 0, y: 0 },
  ]);
  assert.throws(
    () => normalizeLassoSamples(samples, { minDistance: -1 }),
    /minDistance/,
  );
});

test('buildPaddedSelectionBounds enforces minimum content size, padding, and display containment', () => {
  const selection = buildPaddedSelectionBounds(
    [{ x: 97, y: 5 }],
    { x: 0, y: 0, width: 100, height: 100 },
    { minWidth: 20, minHeight: 20, padding: 5 },
  );

  assert.deepEqual(selection, { x: 70, y: 0, width: 30, height: 30 });

  const nonDegenerate = buildPaddedSelectionBounds(
    [{ x: 20, y: 20 }, { x: 30, y: 40 }],
    { x: 0, y: 0, width: 100, height: 100 },
    { minWidth: 20, minHeight: 20, padding: 3 },
  );
  assert.deepEqual(nonDegenerate, { x: 12, y: 17, width: 26, height: 26 });
});

test('buildPaddedSelectionBounds rejects unusable lassos and invalid display geometry', () => {
  const display = { x: 10, y: 10, width: 100, height: 80 };

  assert.equal(buildPaddedSelectionBounds([], display), null);
  assert.equal(buildPaddedSelectionBounds([{ x: -20, y: -20 }], display), null);
  assert.equal(buildPaddedSelectionBounds([{ x: 200, y: 200 }], display), null);
  assert.deepEqual(
    buildPaddedSelectionBounds(
      [{ x: -100, y: 20 }, { x: 500, y: 70 }],
      display,
      { minWidth: 0, minHeight: 0, padding: 0 },
    ),
    { x: 10, y: 20, width: 100, height: 50 },
  );
  assert.throws(
    () => buildPaddedSelectionBounds([{ x: 20, y: 20 }], { x: 0, y: 0, width: 0, height: 10 }),
    /greater than zero/,
  );
});

test('mapDipRectToScreenshotPixels derives physical pixels from Electron display scale', () => {
  const mapped = mapDipRectToScreenshotPixels(
    { x: 110, y: 60, width: 100, height: 50 },
    { x: 10, y: 20, width: 1000, height: 500, scaleFactor: 2 },
  );

  assert.deepEqual(mapped, {
    x: 200,
    y: 80,
    width: 200,
    height: 100,
    scaleX: 2,
    scaleY: 2,
    sourceWidth: 2000,
    sourceHeight: 1000,
  });
});

test('mapDipRectToScreenshotPixels uses actual screenshot dimensions and safely handles clipping and points', () => {
  const mapped = mapDipRectToScreenshotPixels(
    { x: -10, y: 10, width: 30, height: 20 },
    { x: 0, y: 0, width: 100, height: 50, scaleFactor: 2 },
    { width: 150, height: 100 },
  );
  assert.deepEqual(mapped, {
    x: 0,
    y: 20,
    width: 30,
    height: 40,
    scaleX: 1.5,
    scaleY: 2,
    sourceWidth: 150,
    sourceHeight: 100,
  });

  assert.deepEqual(
    mapDipRectToScreenshotPixels(
      { x: 99, y: 50, width: 0, height: 0 },
      { x: 0, y: 0, width: 100, height: 100, scaleFactor: 2 },
    ),
    {
      x: 198,
      y: 100,
      width: 1,
      height: 1,
      scaleX: 2,
      scaleY: 2,
      sourceWidth: 200,
      sourceHeight: 200,
    },
  );
  assert.equal(
    mapDipRectToScreenshotPixels(
      { x: 101, y: 0, width: 10, height: 10 },
      { x: 0, y: 0, width: 100, height: 100 },
    ),
    null,
  );
  assert.equal(
    mapDipRectToScreenshotPixels(
      { x: 100, y: 10, width: 10, height: 10 },
      { x: 0, y: 0, width: 100, height: 100 },
    ),
    null,
  );
});

test('planCappedCrop clips outward, applies both dimension and pixel caps, and never upscales', () => {
  const dimensionCapped = planCappedCrop(
    { x: -5, y: 10, width: 120, height: 100 },
    { width: 100, height: 100 },
    { maxWidth: 50, maxHeight: 30, maxPixels: Infinity },
  );
  assert.deepEqual(dimensionCapped, {
    sourceRect: { x: 0, y: 10, width: 100, height: 90 },
    outputSize: { width: 33, height: 30 },
    scaleX: 0.33,
    scaleY: 1 / 3,
    downscaled: true,
  });

  const pixelCapped = planCappedCrop(
    { x: 0, y: 0, width: 2000, height: 1000 },
    { width: 2000, height: 1000 },
    { maxWidth: 1600, maxHeight: 1600, maxPixels: 500_000 },
  );
  assert.deepEqual(pixelCapped.outputSize, { width: 1000, height: 500 });
  assert.ok(pixelCapped.outputSize.width * pixelCapped.outputSize.height <= 500_000);

  const unchanged = planCappedCrop(
    { x: 10, y: 10, width: 100, height: 50 },
    { width: 200, height: 100 },
    { maxWidth: 500, maxHeight: 500, maxPixels: 1_000_000 },
  );
  assert.deepEqual(unchanged.outputSize, { width: 100, height: 50 });
  assert.equal(unchanged.downscaled, false);
});

test('planCappedCrop resolves extreme thin crops without exceeding a one-pixel cap', () => {
  const plan = planCappedCrop(
    { x: 0, y: 0, width: 1, height: 100 },
    { width: 1, height: 100 },
    { maxWidth: 100, maxHeight: 100, maxPixels: 1 },
  );

  assert.deepEqual(plan.outputSize, { width: 1, height: 1 });
  assert.equal(plan.outputSize.width * plan.outputSize.height, 1);
  assert.equal(planCappedCrop({ x: 200, y: 0, width: 2, height: 2 }, { width: 10, height: 10 }), null);
  assert.throws(
    () => planCappedCrop({ x: 0, y: 0, width: 1, height: 1 }, { width: 1, height: 1 }, { maxPixels: 0 }),
    /maxPixels/,
  );
});
