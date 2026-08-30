// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

/* global wasm_bindgen */

(() => {
  "use strict";

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

  async function start() {
    const payload = window.GWR_VISUALISATION_PAYLOAD;
    if (!payload) {
      throw new Error("Report payload is missing");
    }
    const module = WebAssembly.compile(decodeBase64(payload.wasm));
    const data = decompressGzip(decodeBase64(payload.data));
    await wasm_bindgen({ module_or_path: await module });
    wasm_bindgen.run(await data);
    await waitForSummaryRender();
    markSummaryReady();
    const tensors = await decompressGzip(decodeBase64(payload.tensors));
    wasm_bindgen.load_tensors(tensors);
    markApplicationReady();
    delete window.GWR_VISUALISATION_PAYLOAD;
  }

  start().catch(showError);
})();
