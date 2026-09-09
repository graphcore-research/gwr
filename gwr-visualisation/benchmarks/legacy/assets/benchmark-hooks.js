// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

(() => {
  "use strict";

  const App = window.GWR_VISUALISATION_APP;

  function filtering(iterations) {
    let checksum = 0;
    for (let index = 0; index < iterations; index += 1) {
      App.filterContextCache.clear();
      const context = App.contextSnapshot();
      checksum += context.dataEdges + context.tensors.length;
    }
    return checksum;
  }

  function aggregation(iterations) {
    let checksum = 0;
    for (let index = 0; index < iterations; index += 1) {
      App.filterContextCache.clear();
      const summary = App.filteredSummary();
      checksum += summary.computeNodes + Number(summary.machineOps.total);
    }
    return checksum;
  }

  function geometry(iterations) {
    let checksum = 0;
    for (let index = 0; index < iterations; index += 1) {
      const regions = App.buildTensorRegions(App.data.tensors || [], true);
      checksum += regions.length;
    }
    return checksum;
  }

  const kernels = { filtering, aggregation, geometry };
  window.GWR_BENCHMARK_KERNELS = {
    run(name, iterations) {
      return kernels[name](iterations);
    },
  };
  window.GWR_BENCHMARK_FLUSH = () => App.flushRender();
  window.setTimeout(() => {
    document.body.offsetHeight;
    performance.mark("gwr-initial-summary-ready");
    document.documentElement.dataset.gwrReady = "complete";
  }, 0);
})();
