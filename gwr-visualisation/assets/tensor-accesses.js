// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

(() => {
  const App = window.GWR_VISUALISATION_APP;
  const {
    fmt,
    state,
    tensorAccesses,
    computeNodes,
    pesByName,
    filterPickers,
    layerFilterValue,
    peFilterValue,
    filterMatches,
    bindSelectAndFilter,
    selectOnlyFilterValue,
    markEntityElement,
    toBigInt,
    ratioPercent,
    formatBytes,
    formatHex,
    escapeHtml,
  } = App;

  const ACCESS_PAGE_ROW_LIMIT = 200;
  const MAX_COVERAGE_RUNS = 4096;
  let accessPage = 0;
  let physicalRunLimit = 8;
  let renderedTensorId = null;
  let renderedAccessPage = null;

  function accessScrollState() {
    const groups = tensorAccesses.querySelector(".tensor-access-groups");
    return {
      listLeft: groups?.scrollLeft || 0,
      listTop: groups?.scrollTop || 0,
      windowX: window.scrollX,
      windowY: window.scrollY,
    };
  }

  function restoreAccessScroll(scrollState) {
    const groups = tensorAccesses.querySelector(".tensor-access-groups");
    if (groups) {
      groups.scrollLeft = scrollState.listLeft;
      groups.scrollTop = scrollState.listTop;
    }
    window.scrollTo(scrollState.windowX, scrollState.windowY);
    window.requestAnimationFrame(() =>
      window.scrollTo(scrollState.windowX, scrollState.windowY),
    );
  }

  function accessKey(tensor, access, accessIndex) {
    return `${tensor.id}:${access.direction}:${access.node}:${access.slot ?? "implicit"}:${accessIndex}`;
  }

  function accessSlot(access) {
    return access.slot === undefined ? "unspecified" : fmt.format(access.slot);
  }

  function accessGeometry(tensor, access) {
    const view = access.view === undefined ? null : tensor.views?.[access.view];
    return {
      offsets: view?.offsets || tensor.shape.map(() => 0),
      shape: view?.shape || tensor.shape,
      byteOffset: toBigInt(view?.byte_offset || 0),
      numBytes: toBigInt(view?.num_bytes || tensor.num_bytes),
      full: !view,
    };
  }

  function product(values) {
    return values.reduce((result, value) => result * BigInt(value), 1n);
  }

  function formatByteOffset(value) {
    return `${fmt.format(value)} B`;
  }

  function coverageLayout(tensor, geometry) {
    const totalElements = product(tensor.shape);
    if (geometry.full || tensor.shape.length === 0) {
      return {
        prefixShape: [],
        runCount: 1n,
        runLength: totalElements,
        totalElements,
      };
    }

    // Full trailing dimensions are contiguous in row-major storage.
    let runDimension = tensor.shape.length - 1;
    while (
      runDimension > 0 &&
      geometry.offsets[runDimension] === 0 &&
      geometry.shape[runDimension] === tensor.shape[runDimension]
    ) {
      runDimension -= 1;
    }
    const prefixShape = geometry.shape.slice(0, runDimension);
    return {
      prefixShape,
      runCount: product(prefixShape),
      runLength: product(geometry.shape.slice(runDimension)),
      totalElements,
    };
  }

  function coverageRunAt(tensor, geometry, layout, ordinal) {
    const coordinates = new Array(layout.prefixShape.length);
    for (
      let dimension = layout.prefixShape.length - 1;
      dimension >= 0;
      dimension -= 1
    ) {
      const extent = BigInt(layout.prefixShape[dimension]);
      coordinates[dimension] = Number(ordinal % extent);
      ordinal /= extent;
    }

    let start = 0n;
    for (let dimension = 0; dimension < tensor.shape.length; dimension += 1) {
      const coordinate =
        dimension < layout.prefixShape.length
          ? geometry.offsets[dimension] + coordinates[dimension]
          : geometry.offsets[dimension];
      start = start * BigInt(tensor.shape[dimension]) + BigInt(coordinate);
    }
    return { start, end: start + layout.runLength };
  }

  function sampledCoverageRuns(tensor, geometry) {
    const layout = coverageLayout(tensor, geometry);
    const sampleCount = Number(
      layout.runCount < BigInt(MAX_COVERAGE_RUNS)
        ? layout.runCount
        : BigInt(MAX_COVERAGE_RUNS),
    );
    const runs = [];
    for (let sample = 0; sample < sampleCount; sample += 1) {
      const ordinal =
        (BigInt(sample * 2 + 1) * layout.runCount) / BigInt(sampleCount * 2);
      runs.push({
        ...coverageRunAt(tensor, geometry, layout, ordinal),
        weight: Number(layout.runCount) / sampleCount,
      });
    }
    return runs;
  }

  function physicalByteRange(tensor, run) {
    const totalElements = product(tensor.shape);
    const inferredBits = (toBigInt(tensor.num_bytes) * 8n) / totalElements;
    const elementBits = BigInt(tensor.element_bits || inferredBits || 1n);
    const startBit = run.start * elementBits;
    const endBit = run.end * elementBits;
    return {
      start: startBit / 8n,
      end: (endBit + 7n) / 8n,
    };
  }

  function drawCoverage(canvas, tensor, geometry) {
    const pixelRatio = window.devicePixelRatio || 1;
    const width = Math.max(Math.round(canvas.clientWidth * pixelRatio), 256);
    const height = Math.max(Math.round(canvas.clientHeight * pixelRatio), 1);
    const coverage = new Float64Array(width);
    const totalElements = Number(product(tensor.shape));
    for (const run of sampledCoverageRuns(tensor, geometry)) {
      const start = (Number(run.start) / totalElements) * width;
      const end = (Number(run.end) / totalElements) * width;
      const firstPixel = Math.max(Math.floor(start), 0);
      const lastPixel = Math.min(Math.ceil(end), width);
      for (let pixel = firstPixel; pixel < lastPixel; pixel += 1) {
        const overlap = Math.max(
          Math.min(end, pixel + 1) - Math.max(start, pixel),
          0,
        );
        coverage[pixel] += overlap * run.weight;
      }
    }

    canvas.width = width;
    canvas.height = height;
    const context = canvas.getContext("2d");
    context.fillStyle = getComputedStyle(canvas).color;
    for (let pixel = 0; pixel < width; pixel += 1) {
      const amount = Math.min(coverage[pixel], 1);
      context.globalAlpha = amount === 0 ? 0 : 0.35 + amount * 0.65;
      context.fillRect(pixel, 0, 1, height);
    }
    context.globalAlpha = 1;
  }

  function comparePes(left, right) {
    const leftPe = pesByName.get(left) || {};
    const rightPe = pesByName.get(right) || {};
    return (
      Number(leftPe.row ?? Number.MAX_SAFE_INTEGER) -
        Number(rightPe.row ?? Number.MAX_SAFE_INTEGER) ||
      Number(leftPe.col ?? Number.MAX_SAFE_INTEGER) -
        Number(rightPe.col ?? Number.MAX_SAFE_INTEGER) ||
      left.localeCompare(right, undefined, { numeric: true })
    );
  }

  function compareAccesses(left, right) {
    return (
      (left.access.direction === "read" ? 0 : 1) -
        (right.access.direction === "read" ? 0 : 1) ||
      left.node.id.localeCompare(right.node.id, undefined, { numeric: true }) ||
      (left.access.slot ?? Number.MAX_SAFE_INTEGER) -
        (right.access.slot ?? Number.MAX_SAFE_INTEGER)
    );
  }

  function groupedAccesses(tensor) {
    const groups = new Map();
    for (const [accessIndex, access] of (tensor.accesses || []).entries()) {
      const node = computeNodes[access.node];
      if (
        !node ||
        !filterMatches(layerFilterValue(), node.layer) ||
        !filterMatches(peFilterValue(), node.pe)
      ) {
        continue;
      }
      const item = {
        access,
        node,
        geometry: accessGeometry(tensor, access),
        key: accessKey(tensor, access, accessIndex),
      };
      if (!groups.has(node.pe)) {
        groups.set(node.pe, []);
      }
      groups.get(node.pe).push(item);
    }
    return [...groups]
      .sort(([left], [right]) => comparePes(left, right))
      .map(([pe, accesses]) => ({
        pe,
        accesses: accesses.sort(compareAccesses),
      }));
  }

  function accessPages(groups) {
    const pages = [];
    let page = [];
    let rowCount = 0;
    for (const group of groups) {
      if (
        page.length &&
        rowCount + group.accesses.length > ACCESS_PAGE_ROW_LIMIT
      ) {
        pages.push(page);
        page = [];
        rowCount = 0;
      }
      page.push(group);
      rowCount += group.accesses.length;
    }
    if (page.length) {
      pages.push(page);
    }
    return pages;
  }

  function pageRange(pages, pageIndex) {
    let peStart = 0;
    let accessStart = 0;
    for (let index = 0; index < pageIndex; index += 1) {
      peStart += pages[index].length;
      accessStart += pages[index].reduce(
        (total, group) => total + group.accesses.length,
        0,
      );
    }
    const groups = pages[pageIndex] || [];
    const accesses = groups.reduce(
      (total, group) => total + group.accesses.length,
      0,
    );
    return {
      accesses,
      accessStart: accesses ? accessStart + 1 : 0,
      accessEnd: accessStart + accesses,
      peStart: groups.length ? peStart + 1 : 0,
      peEnd: peStart + groups.length,
    };
  }

  function syncSelectedAccess(tensor, groups) {
    const accesses = groups.flatMap((group) => group.accesses);
    if (tensor.id !== renderedTensorId) {
      renderedTensorId = tensor.id;
      state.selectedTensorAccessKey = null;
      accessPage = 0;
    }
    let selected = accesses.find(
      (item) => item.key === state.selectedTensorAccessKey,
    );
    if (!selected) {
      selected = accesses[0] || null;
      state.selectedTensorAccessKey = selected?.key || null;
      accessPage = 0;
    }
    return selected;
  }

  function physicalRunsMarkup(tensor, geometry, direction) {
    const layout = coverageLayout(tensor, geometry);
    const visibleCount = Number(
      layout.runCount < BigInt(physicalRunLimit)
        ? layout.runCount
        : BigInt(physicalRunLimit),
    );
    const runs = Array.from({ length: visibleCount }, (_, index) => {
      const run = coverageRunAt(tensor, geometry, layout, BigInt(index));
      return { ...run, bytes: physicalByteRange(tensor, run) };
    });
    const firstBytes = runs[0]?.bytes;
    const runBytes = firstBytes ? firstBytes.end - firstBytes.start : 0n;
    const stride =
      runs.length > 1 ? runs[1].bytes.start - runs[0].bytes.start : null;
    const rows = runs
      .map(
        (run, index) => `
          <div class="tensor-physical-run">
            <span>#${fmt.format(index + 1)}</span>
            <div class="tensor-view-track ${direction}" role="img" aria-label="Physical run ${fmt.format(index + 1)}: tensor bytes ${formatByteOffset(run.bytes.start)} to ${formatByteOffset(run.bytes.end)}">
              <i style="left: ${ratioPercent(run.start, layout.totalElements)}%; width: ${ratioPercent(run.end - run.start, layout.totalElements)}%"></i>
            </div>
            <strong>${formatByteOffset(run.bytes.start)}–${formatByteOffset(run.bytes.end)}</strong>
          </div>`,
      )
      .join("");
    return `
      <div class="tensor-physical-runs">
        <div class="tensor-physical-runs-summary">
          <strong>First ${fmt.format(visibleCount)} of ${fmt.format(layout.runCount)} physical runs</strong>
          <span>${formatBytes(runBytes)} each${stride === null ? "" : ` · ${formatByteOffset(stride)} start stride`}</span>
        </div>
        ${rows}
      </div>`;
  }

  function selectedViewMarkup(tensor, selected) {
    if (!selected) {
      return `<p class="tensor-access-empty">No compute-node accesses match the current Layer and PE filters.</p>`;
    }
    const { access, node, geometry } = selected;
    const direction = access.direction === "read" ? "Read" : "Written";
    const slot = access.direction === "read" ? "Input" : "Output";
    const tensorBytes = toBigInt(tensor.num_bytes);
    const percentage = ratioPercent(geometry.numBytes, tensorBytes);
    const absoluteStart = toBigInt(tensor.addr) + geometry.byteOffset;
    const dimensionRows = tensor.shape
      .map((dimension, index) => {
        const offset = Number(geometry.offsets[index] || 0);
        const extent = Number(geometry.shape[index] || 0);
        return `
          <div class="tensor-view-dimension">
            <span>D${index}</span>
            <div class="tensor-view-track" role="img" aria-label="Dimension ${index}: offset ${fmt.format(offset)}, extent ${fmt.format(extent)}, total ${fmt.format(dimension)}">
              <i style="left: ${ratioPercent(offset, dimension)}%; width: ${ratioPercent(extent, dimension)}%"></i>
            </div>
            <strong>${fmt.format(offset)}–${fmt.format(offset + extent)} <em>of ${fmt.format(dimension)}</em></strong>
          </div>`;
      })
      .join("");
    return `
      <section class="tensor-view-selection ${access.direction}">
        <div class="tensor-view-selection-heading">
          <strong>${direction} · ${escapeHtml(node.pe)} · ${escapeHtml(node.id)}</strong>
          <span>${escapeHtml(node.op)} · ${escapeHtml(node.layer)} · ${slot} ${accessSlot(access)}</span>
          <span>${formatBytes(geometry.numBytes)} · ${percentage.toFixed(1)}% · ${formatHex(absoluteStart)}</span>
        </div>
        <div class="tensor-view-dimensions">${dimensionRows}</div>
        ${physicalRunsMarkup(tensor, geometry, access.direction)}
      </section>`;
  }

  function groupTotals(group) {
    return group.accesses.reduce(
      (totals, item) => {
        totals[item.access.direction].count += 1;
        totals[item.access.direction].bytes += item.geometry.numBytes;
        return totals;
      },
      {
        read: { count: 0, bytes: 0n },
        write: { count: 0, bytes: 0n },
      },
    );
  }

  function accessRowMarkup(tensor, item) {
    const { access, node, geometry, key } = item;
    const direction = access.direction === "read" ? "Read" : "Written";
    const slot = access.direction === "read" ? "Input" : "Output";
    const shape = geometry.shape.join(" × ");
    const offsets = geometry.offsets.join(", ");
    const selected = key === state.selectedTensorAccessKey;
    return `
      <button type="button" class="tensor-access-row ${access.direction}${selected ? " selected-access" : ""}" data-access-key="${escapeHtml(key)}" data-pe="${escapeHtml(node.pe)}" aria-pressed="${selected}" aria-label="${escapeHtml(`${direction} by ${node.id} on ${node.pe}, ${formatBytes(geometry.numBytes)}, ${slot} ${accessSlot(access)}`)}">
        <span class="tensor-access-direction">${direction}</span>
        <span class="tensor-access-node"><strong>${escapeHtml(node.id)}</strong><em>${escapeHtml(node.op)} · ${escapeHtml(node.layer)} · ${slot} ${accessSlot(access)}</em></span>
        <span class="tensor-access-view"><strong>${geometry.full ? "Full tensor" : `[${escapeHtml(shape)}]`}</strong><em>${geometry.full ? "offsets [0]" : `offsets [${escapeHtml(offsets)}]`}</em></span>
        <span class="tensor-access-bytes">${formatBytes(geometry.numBytes)}</span>
        <canvas class="tensor-access-coverage ${access.direction}" data-coverage-access="${escapeHtml(key)}" role="img" aria-label="${direction} physical coverage: ${formatBytes(geometry.numBytes)}, first byte at tensor offset ${formatBytes(geometry.byteOffset)}"></canvas>
      </button>`;
  }

  function drawCoverageLanes(container, tensor, groups) {
    const itemsByKey = new Map(
      groups.flatMap((group) => group.accesses).map((item) => [item.key, item]),
    );
    for (const canvas of container.querySelectorAll("[data-coverage-access]")) {
      const item = itemsByKey.get(canvas.dataset.coverageAccess);
      if (item) {
        drawCoverage(canvas, tensor, item.geometry);
      }
    }
  }

  function bindAccessRows(container, tensor, groups) {
    const itemsByKey = new Map(
      groups.flatMap((group) => group.accesses).map((item) => [item.key, item]),
    );
    for (const row of container.querySelectorAll("[data-access-key]")) {
      const item = itemsByKey.get(row.dataset.accessKey);
      if (!item) {
        continue;
      }
      markEntityElement(row, "pe", item.node.pe);
      row.addEventListener("click", () => {
        state.selectedTensorAccessKey = item.key;
        App.selectCompute(item.node);
      });
      row.addEventListener("dblclick", (event) => {
        event.preventDefault();
        selectOnlyFilterValue(filterPickers.pes, item.node.pe);
      });
    }
  }

  function renderAccessGroups(container, tensor, groups) {
    for (const group of groups) {
      const section = document.createElement("section");
      section.className = "tensor-access-group";
      const totals = groupTotals(group);
      const header = document.createElement("button");
      header.type = "button";
      header.className = "tensor-access-pe";
      const selectedPe = group.pe === state.selectedPe?.name;
      header.classList.toggle("selected", selectedPe);
      header.setAttribute("aria-pressed", selectedPe ? "true" : "false");
      header.innerHTML = `
        <strong>${escapeHtml(group.pe)}</strong>
        <span>Read ${fmt.format(totals.read.count)} · ${formatBytes(totals.read.bytes)}</span>
        <span>Written ${fmt.format(totals.write.count)} · ${formatBytes(totals.write.bytes)}</span>`;
      bindSelectAndFilter(
        header,
        () => {
          App.selectPe(pesByName.get(group.pe));
        },
        filterPickers.pes,
        group.pe,
      );
      section.innerHTML = `
        <div class="tensor-access-columns" aria-hidden="true">
          <span>Direction</span><span>Compute node</span><span>TensorView</span><span>Bytes</span><span>Tensor coverage</span>
        </div>
        <div class="tensor-access-rows">
          ${group.accesses.map((item) => accessRowMarkup(tensor, item)).join("")}
        </div>`;
      section.prepend(header);
      container.append(section);
    }
    bindAccessRows(container, tensor, groups);
  }

  function renderTensorAccesses() {
    if (tensorAccesses.closest("[data-view]")?.hidden) {
      return;
    }
    const tensor = state.selectedTensor;
    if (!tensor) {
      tensorAccesses.textContent = "No tensor selected.";
      return;
    }
    const scrollState = accessScrollState();
    const previousTensorId = renderedTensorId;
    const previousAccessPage = renderedAccessPage;
    const groups = groupedAccesses(tensor);
    const selected = syncSelectedAccess(tensor, groups);
    const pages = accessPages(groups);
    accessPage = Math.min(accessPage, Math.max(pages.length - 1, 0));
    const visibleGroups = pages[accessPage] || [];
    const range = pageRange(pages, accessPage);
    const totalRows = groups.reduce(
      (total, group) => total + group.accesses.length,
      0,
    );
    const preserveScroll =
      tensor.id === previousTensorId && accessPage === previousAccessPage;

    tensorAccesses.innerHTML = `
      <div class="tensor-access-status">
        <span>${escapeHtml(tensor.id)} · ${fmt.format(totalRows)} compute-node accesses across ${fmt.format(groups.length)} PEs</span>
        <span class="tensor-access-legend"><i class="read"></i>Read <i class="write"></i>Written</span>
        <label class="tensor-access-run-limit">Physical runs
          <select data-physical-run-limit aria-label="Physical runs to show">
            ${[4, 8, 16, 32, 64].map((limit) => `<option value="${limit}"${limit === physicalRunLimit ? " selected" : ""}>First ${limit}</option>`).join("")}
          </select>
        </label>
        <span>Page ${fmt.format(accessPage + 1)} of ${fmt.format(Math.max(pages.length, 1))} · PEs ${fmt.format(range.peStart)}–${fmt.format(range.peEnd)} of ${fmt.format(groups.length)} · accesses ${fmt.format(range.accessStart)}–${fmt.format(range.accessEnd)} of ${fmt.format(totalRows)}</span>
        <button type="button" data-access-page="previous"${accessPage === 0 ? " disabled" : ""}>Previous</button>
        <button type="button" data-access-page="next"${accessPage >= pages.length - 1 ? " disabled" : ""}>Next</button>
      </div>
      ${selectedViewMarkup(tensor, selected)}
      <div class="tensor-access-groups"></div>`;

    renderAccessGroups(
      tensorAccesses.querySelector(".tensor-access-groups"),
      tensor,
      visibleGroups,
    );
    drawCoverageLanes(tensorAccesses, tensor, groups);
    tensorAccesses
      .querySelector("[data-physical-run-limit]")
      .addEventListener("change", (event) => {
        physicalRunLimit = Number(event.currentTarget.value);
        const runs = tensorAccesses.querySelector(".tensor-physical-runs");
        if (runs && selected) {
          runs.outerHTML = physicalRunsMarkup(
            tensor,
            selected.geometry,
            selected.access.direction,
          );
        }
      });
    for (const button of tensorAccesses.querySelectorAll(
      "[data-access-page]",
    )) {
      button.addEventListener("click", () => {
        accessPage += button.dataset.accessPage === "next" ? 1 : -1;
        renderTensorAccesses();
      });
    }
    renderedAccessPage = accessPage;
    if (preserveScroll) {
      restoreAccessScroll(scrollState);
    }
  }

  function resetTensorAccessPage() {
    accessPage = 0;
  }

  Object.assign(App, {
    renderTensorAccesses,
    resetTensorAccessPage,
  });
})();
