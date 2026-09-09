// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

(() => {
  "use strict";

  const applicationScripts = [
    "view-model.js",
    "core.js",
    "filters.js",
    "pe-grid.js",
    "timetable.js",
    "tensors.js",
    "memory.js",
    "relationships.js",
    "workspace.js",
    "app.js",
  ];

  function decodeBase64(value) {
    if (typeof Uint8Array.fromBase64 === "function") {
      return Uint8Array.fromBase64(value);
    }
    const binary = window.atob(value);
    const bytes = new Uint8Array(binary.length);
    for (let index = 0; index < binary.length; index += 1) {
      bytes[index] = binary.charCodeAt(index);
    }
    return bytes;
  }

  async function decompressGzip(bytes) {
    if (typeof DecompressionStream !== "function") {
      throw new Error("This browser does not support gzip decompression");
    }
    const stream = new Blob([bytes])
      .stream()
      .pipeThrough(new DecompressionStream("gzip"));
    return new Uint8Array(await new Response(stream).arrayBuffer());
  }

  function showError(error) {
    const warnings = document.getElementById("warnings");
    const message = document.createElement("p");
    message.textContent = `Unable to start visualisation: ${error}`;
    warnings.replaceChildren(message);
    document.documentElement.dataset.gwrError = String(error);
  }

  function markSummaryReady() {
    performance.mark("gwr-initial-summary-ready");
    document.documentElement.dataset.gwrSummaryReady = "complete";
  }

  function waitForSummaryRender() {
    return new Promise((resolve) => {
      window.setTimeout(() => {
        document.body.offsetHeight;
        resolve();
      }, 0);
    });
  }

  function markApplicationReady() {
    document.documentElement.dataset.gwrReady = "complete";
  }

  function loadScript(source) {
    return new Promise((resolve, reject) => {
      const script = document.createElement("script");
      script.src = source;
      script.addEventListener("load", resolve, { once: true });
      script.addEventListener(
        "error",
        () => reject(new Error(`Unable to load ${source}`)),
        { once: true },
      );
      document.body.append(script);
    });
  }

  async function start() {
    const payload = window.GWR_VISUALISATION_PAYLOAD;
    if (!payload) {
      throw new Error("Report payload is missing");
    }
    const [dataJson, tensorsJson] = await Promise.all([
      decompressGzip(decodeBase64(payload.data)),
      decompressGzip(decodeBase64(payload.tensors)),
    ]);
    const decoder = new TextDecoder();
    const data = JSON.parse(decoder.decode(dataJson));
    data.tensors = JSON.parse(decoder.decode(tensorsJson));
    window.GWR_VISUALISATION_DATA = data;
    for (const script of [
      ...applicationScripts,
      ...(window.GWR_VISUALISATION_SCRIPTS || []),
    ]) {
      await loadScript(script);
    }
    await waitForSummaryRender();
    markSummaryReady();
    markApplicationReady();
    delete window.GWR_VISUALISATION_PAYLOAD;
    delete window.GWR_VISUALISATION_SCRIPTS;
  }

  start().catch(showError);
})();
