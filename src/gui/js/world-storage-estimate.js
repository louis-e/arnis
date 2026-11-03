(function (root, factory) {
  const api = factory();
  if (typeof module === "object" && module.exports) {
    module.exports = api;
  } else {
    root.WorldStorageEstimate = api;
  }
})(typeof globalThis !== "undefined" ? globalThis : this, function () {
  const GB = 1_000_000_000;
  const MB = 1_000_000;
  const MAX_WORLD_STORAGE_BYTES = 120 * GB;
  const UNCERTAINTY_MULTIPLIER = 2;

  // These are measured full Arnis world outputs for square selections over
  // central Munich, recorded with the area-warning calibration in main.js.
  // The output size includes all chunks and generated features from those runs.
  const CALIBRATION_SAMPLES = Object.freeze([
    Object.freeze({ areaKm2: 9, outputBytes: 146 * MB }),
    Object.freeze({ areaKm2: 25, outputBytes: 399 * MB }),
    Object.freeze({ areaKm2: 85, outputBytes: 1_300 * MB }),
    Object.freeze({ areaKm2: 150, outputBytes: 2_300 * MB }),
  ]);

  function fitLinearTrend(samples) {
    const count = samples.length;
    const meanArea = samples.reduce((sum, sample) => sum + sample.areaKm2, 0) / count;
    const meanBytes = samples.reduce((sum, sample) => sum + sample.outputBytes, 0) / count;
    let covariance = 0;
    let variance = 0;
    for (const sample of samples) {
      const areaDelta = sample.areaKm2 - meanArea;
      covariance += areaDelta * (sample.outputBytes - meanBytes);
      variance += areaDelta * areaDelta;
    }
    const bytesPerKm2 = covariance / variance;
    return {
      fixedBytes: Math.max(0, meanBytes - bytesPerKm2 * meanArea),
      bytesPerKm2,
    };
  }

  const TREND = fitLinearTrend(CALIBRATION_SAMPLES);
  // Use the highest observed output rate instead of the regression slope. The
  // estimate therefore covers every measured point before the separate 2x
  // allowance for map density, output format, and option variability.
  const MAX_OBSERVED_BYTES_PER_KM2 = Math.max(
    ...CALIBRATION_SAMPLES.map((sample) => sample.outputBytes / sample.areaKm2),
  );

  function estimateWorldStorageBytes({ areaM2, body = "earth", scale = 1, existingBytes = 0 } = {}) {
    if (!Number.isFinite(areaM2) || areaM2 <= 0) return null;

    const validScale = Number.isFinite(scale) && scale > 0 ? scale : 1;
    const scaleFactor = body === "earth" ? validScale * validScale : 1;
    const scaledAreaKm2 = (areaM2 / 1_000_000) * scaleFactor;
    const existingSize = Number.isFinite(existingBytes) && existingBytes > 0
      ? existingBytes
      : 0;
    const measuredBaseline = TREND.fixedBytes + scaledAreaKm2 * MAX_OBSERVED_BYTES_PER_KM2;
    const estimate = Math.ceil(existingSize + measuredBaseline * UNCERTAINTY_MULTIPLIER);

    return Math.min(Number.MAX_SAFE_INTEGER, estimate);
  }

  function generationButtonState(estimateBytes, generationEnabled, hasSelection = Number.isFinite(estimateBytes)) {
    const exceedsLimit = Number.isFinite(estimateBytes) && estimateBytes > MAX_WORLD_STORAGE_BYTES;
    const unavailable = hasSelection && !Number.isFinite(estimateBytes);
    return {
      disabled: generationEnabled !== true || exceedsLimit || unavailable,
      exceedsLimit,
      unavailable,
    };
  }

  function formatBytes(bytes) {
    if (!Number.isFinite(bytes) || bytes < 0) return "";
    if (bytes >= GB) {
      const digits = bytes >= 10 * GB ? 1 : 2;
      return `${(bytes / GB).toFixed(digits)} GB`;
    }
    return `${(bytes / MB).toFixed(bytes >= 100 * MB ? 0 : 1)} MB`;
  }

  return Object.freeze({
    CALIBRATION_SAMPLES,
    MAX_WORLD_STORAGE_BYTES,
    UNCERTAINTY_MULTIPLIER,
    TREND,
    MAX_OBSERVED_BYTES_PER_KM2,
    estimateWorldStorageBytes,
    generationButtonState,
    formatBytes,
  });
});
