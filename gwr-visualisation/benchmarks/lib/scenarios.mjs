// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

export const metricLabels = {
  cold_startup_ms: "Cold startup",
  layers_preset_ms: "Layers preset",
  compute_preset_ms: "Compute preset",
  regex_filter_ms: "Regex filter + select",
  relationships_ms: "Relationship build + render",
  kernel_filtering_ms: "Filtering kernel",
  kernel_aggregation_ms: "Aggregation kernel",
  kernel_geometry_ms: "Geometry kernel",
};

export async function runBenchmarkScenario(
  session,
  kernelIterations,
  interactionLayerPattern,
  progress = () => {},
) {
  const cold = await session.coldStartup();
  if (!Number.isFinite(cold)) {
    throw new Error(
      "The report did not publish its initial Summary render mark",
    );
  }
  progress("cold startup", cold);
  await selectRepresentativeLayers(session, interactionLayerPattern);
  const metrics = {
    cold_startup_ms: cold,
  };
  metrics.layers_preset_ms = await measure(
    session,
    "Layers preset",
    `document.querySelector('[data-preset="layers"]').click()`,
    progress,
  );
  metrics.compute_preset_ms = await measure(
    session,
    "Compute preset",
    `document.querySelector('[data-preset="compute"]').click()`,
    progress,
  );
  metrics.regex_filter_ms = await measure(
    session,
    "Regex filter + select",
    regexFilterAction(interactionLayerPattern),
    progress,
  );
  metrics.relationships_ms = await measure(
    session,
    "Relationship build + render",
    relationshipAction(),
    progress,
  );
  const checksums = {};
  for (const name of ["filtering", "aggregation", "geometry"]) {
    const result = await session.measureKernel(name, kernelIterations);
    if (!Number.isFinite(result.checksum)) {
      throw new Error(`${name} kernel returned an invalid checksum`);
    }
    metrics[`kernel_${name}_ms`] = result.milliseconds;
    checksums[name] = result.checksum;
    progress(`${name} kernel`, result.milliseconds);
  }
  return { metrics, checksums };
}

async function selectRepresentativeLayers(session, pattern) {
  await session.evaluate(`(() => { ${regexFilterAction(pattern)} })()`);
}

async function measure(session, label, action, progress) {
  const milliseconds = await session.measureAction(action);
  progress(label, milliseconds);
  return milliseconds;
}

function regexFilterAction(pattern) {
  return `
    const details = document.getElementById('layer-filter').closest('details');
    details.open = true;
    details.dispatchEvent(new Event('toggle'));
    const pattern = document.getElementById('layer-filter-pattern');
    pattern.value = ${JSON.stringify(pattern)};
    pattern.dispatchEvent(new Event('input', { bubbles: true }));
    document.getElementById('layer-filter-select-matches').click();
  `;
}

function relationshipAction() {
  return `
    document.querySelector('[data-preset="layers"]').click();
    const mode = document.getElementById('relationship-mode');
    mode.value = 'tensor-pe';
    mode.dispatchEvent(new Event('change', { bubbles: true }));
  `;
}
