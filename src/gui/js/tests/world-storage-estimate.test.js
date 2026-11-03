const assert = require('node:assert/strict');
const test = require('node:test');

const {
  CALIBRATION_SAMPLES,
  MAX_OBSERVED_BYTES_PER_KM2,
  MAX_WORLD_STORAGE_BYTES,
  TREND,
  estimateWorldStorageBytes,
  formatBytes,
  generationButtonState,
} = require('../world-storage-estimate.js');

test('estimate stays above every measured full-pipeline output', () => {
  for (const sample of CALIBRATION_SAMPLES) {
    const estimate = estimateWorldStorageBytes({ areaM2: sample.areaKm2 * 1_000_000 });
    assert.ok(estimate >= sample.outputBytes * 2, `${sample.areaKm2} km² sample`);
  }
});

test('Earth world scale changes the estimated output area quadratically', () => {
  const areaM2 = 2_000_000;
  const atOne = estimateWorldStorageBytes({ areaM2, body: 'earth', scale: 1 });
  const atTwo = estimateWorldStorageBytes({ areaM2, body: 'earth', scale: 2 });
  const moonAtOne = estimateWorldStorageBytes({ areaM2, body: 'moon', scale: 1 });
  const moonAtTwo = estimateWorldStorageBytes({ areaM2, body: 'moon', scale: 2 });

  const fixedBytes = TREND.fixedBytes * 2;
  const areaBytes = (areaM2 / 1_000_000) * MAX_OBSERVED_BYTES_PER_KM2 * 2;
  assert.equal(atOne, Math.ceil(fixedBytes + areaBytes));
  assert.equal(atTwo, Math.ceil(fixedBytes + areaBytes * 4));
  assert.equal(moonAtTwo, moonAtOne);
});

test('existing One World storage is included in the resulting world estimate', () => {
  const areaM2 = 9_000_000;
  const areaOnly = estimateWorldStorageBytes({ areaM2 });
  const existingBytes = 7_000_000_000;
  const combined = estimateWorldStorageBytes({ areaM2, existingBytes });

  assert.equal(combined, areaOnly + existingBytes);
});

test('the 120 GB limit permits equality and blocks any estimate above it', () => {
  assert.equal(generationButtonState(MAX_WORLD_STORAGE_BYTES, true, true).disabled, false);
  assert.equal(generationButtonState(MAX_WORLD_STORAGE_BYTES + 1, true, true).exceedsLimit, true);
  assert.equal(generationButtonState(MAX_WORLD_STORAGE_BYTES + 1, true, true).disabled, true);
});

test('Generate re-enables when an over-limit selection returns under the limit', () => {
  const oversized = generationButtonState(MAX_WORLD_STORAGE_BYTES + 1, true, true);
  const smaller = generationButtonState(MAX_WORLD_STORAGE_BYTES, true, true);

  assert.equal(oversized.disabled, true);
  assert.equal(smaller.disabled, false);
  assert.equal(generationButtonState(1, false, true).disabled, true);
  assert.equal(generationButtonState(null, true, true).unavailable, true);
  assert.equal(generationButtonState(null, true, true).disabled, true);
  assert.equal(generationButtonState(null, true, false).disabled, false);
});

test('estimate display uses MB below 1 GB and GB at and above 1 GB', () => {
  assert.equal(formatBytes(250_000_000), '250 MB');
  assert.equal(formatBytes(1_000_000_000), '1.00 GB');
});
