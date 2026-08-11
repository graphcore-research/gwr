// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

(() => {
  const App = window.GWR_VISUALISATION_APP;
  const {
    data,
    fmt,
    state,
    pesByName,
    tensorsById,
    relationshipBundle,
    relationshipControls,
    relationshipModelCache,
    machineOpTypes,
    option,
    filterPickers,
    selectOnlyFilterValue,
    filteredLayers,
    peFilterValue,
    layerFilterValue,
    memoryFilterValue,
    tensorFilterValue,
    filterMatches,
    tensorsForContext,
    tensorTrafficFor,
    tensorMemoryShare,
    scaleTensorToMemory,
    cacheKey,
    toBigInt,
    bigIntMax,
    bigIntCompare,
    scaleInteger,
    ratioPercent,
    overlapBytes,
    formatCount,
    formatBytes,
    escapeHtml,
  } = App;

  const relationshipMeasures = {
    compute: [
      ["machine-ops", "Machine ops"],
      ["nodes", "Compute nodes"],
      ...machineOpTypes.map((op) => [op.name, op.label]),
    ],
    memory: [
      ["read", "Read"],
      ["write", "Written"],
    ],
    "pe-memory": [
      ["read", "Read"],
      ["write", "Written"],
    ],
    "tensor-memory": [
      ["read", "Read"],
      ["write", "Written"],
    ],
    "tensor-pe": [
      ["read", "Read"],
      ["write", "Written"],
    ],
  };
  const layerOrderByName = new Map(
    (data.layers || []).map((layer, index) => [layer.name, index]),
  );
  const memoryOrderByName = new Map(
    (data.memory?.platform_memories || []).map((memory, index) => [
      memory.name,
      index,
    ]),
  );

  function updateRelationshipMeasureOptions() {
    const previous = relationshipControls.measure.value;
    relationshipControls.measure.innerHTML = "";
    for (const [value, label] of relationshipMeasures[
      relationshipControls.mode.value
    ]) {
      relationshipControls.measure.append(option(value, label));
    }
    if (
      [...relationshipControls.measure.options].some(
        (entry) => entry.value === previous,
      )
    ) {
      relationshipControls.measure.value = previous;
    }
  }

  function relationshipLayers() {
    return filteredLayers();
  }

  function computeRelationshipModel() {
    const measure = relationshipControls.measure.value;
    const sources = [];
    const targetsById = new Map();
    const edges = [];
    for (const layer of relationshipLayers()) {
      sources.push({
        id: layer.name,
        label: layer.name,
        group: layerBand(layer.name),
      });
      for (const layerPe of layer.pes || []) {
        if (!filterMatches(peFilterValue(), layerPe.name)) {
          continue;
        }
        const pe = pesByName.get(layerPe.name) || {};
        const value =
          measure === "nodes"
            ? toBigInt(layerPe.compute_nodes)
            : toBigInt(
                layerPe.machine_ops?.[
                  measure === "machine-ops" ? "total" : measure
                ],
              );
        if (value <= 0n) {
          continue;
        }
        targetsById.set(layerPe.name, {
          id: layerPe.name,
          label: layerPe.name,
          group: `PE row ${pe.row ?? 0}`,
          order: [Number(pe.row || 0), Number(pe.col || 0)],
        });
        edges.push({ source: layer.name, target: layerPe.name, value });
      }
    }
    const targets = [...targetsById.values()].sort(
      (left, right) =>
        left.order[0] - right.order[0] ||
        left.order[1] - right.order[1] ||
        left.id.localeCompare(right.id),
    );
    return {
      sources,
      targets,
      edges,
      sourceLabel: "layers",
      targetLabel: "PEs",
    };
  }

  function relationshipMemoryTargets(memories) {
    return memories.map((memory) => {
      const index = memoryOrderByName.get(memory.name) ?? 0;
      return {
        id: memory.name,
        label: memory.name,
        group: `${memory.kind} ${Math.floor(index / 4) * 4}-${Math.floor(index / 4) * 4 + 3}`,
        order: [memory.base_addr || "0"],
      };
    });
  }

  function relationshipMemories() {
    return (data.memory?.platform_memories || []).filter((memory) =>
      filterMatches(memoryFilterValue(), memory.name),
    );
  }

  function memoryRelationshipEdges(
    source,
    tensors,
    memories,
    measure,
    layerName,
    peName,
  ) {
    const values = new Map();
    for (const tensor of tensors) {
      const tensorBytes = bigIntMax(toBigInt(tensor.num_bytes), 1n);
      const traffic = tensorTrafficFor(tensor, layerName, peName);
      const trafficBytes =
        measure === "read" ? traffic?.readBytes : traffic?.writtenBytes;
      for (const memory of memories) {
        const overlap = overlapBytes(
          tensor.addr,
          tensorBytes,
          memory.base_addr,
          memory.capacity_bytes,
        );
        if (overlap === 0n) {
          continue;
        }
        const value = scaleInteger(trafficBytes, overlap, tensorBytes);
        values.set(memory.name, toBigInt(values.get(memory.name)) + value);
      }
    }
    return [...values]
      .filter(([, value]) => value > 0n)
      .map(([target, value]) => ({ source, target, value }));
  }

  function memoryRelationshipModel() {
    const measure = relationshipControls.measure.value;
    const memories = relationshipMemories();
    const targets = relationshipMemoryTargets(memories);
    const sources = [];
    const edges = [];
    for (const layer of relationshipLayers()) {
      sources.push({
        id: layer.name,
        label: layer.name,
        group: layerBand(layer.name),
      });
      const tensors = tensorsForContext(layer.name, peFilterValue());
      edges.push(
        ...memoryRelationshipEdges(
          layer.name,
          tensors,
          memories,
          measure,
          layer.name,
          peFilterValue(),
        ),
      );
    }
    return {
      sources,
      targets,
      edges,
      sourceLabel: "layers",
      targetLabel: "memories",
    };
  }

  function peMemoryRelationshipModel() {
    const measure = relationshipControls.measure.value;
    const memories = relationshipMemories();
    const targets = relationshipMemoryTargets(memories);
    const pes = data.pes
      .filter((pe) => filterMatches(peFilterValue(), pe.name))
      .sort(
        (left, right) =>
          left.row - right.row ||
          left.col - right.col ||
          left.name.localeCompare(right.name),
      );
    const sources = pes.map((pe) => ({
      id: pe.name,
      label: pe.name,
      group: `PE row ${pe.row}`,
      order: [pe.row, pe.col],
    }));
    const edges = [];
    for (const pe of pes) {
      const tensors = tensorsForContext(layerFilterValue(), pe.name);
      edges.push(
        ...memoryRelationshipEdges(
          pe.name,
          tensors,
          memories,
          measure,
          layerFilterValue(),
          pe.name,
        ),
      );
    }
    return {
      sources,
      targets,
      edges,
      sourceLabel: "PEs",
      targetLabel: "memories",
    };
  }

  function tensorRelationshipLayer(tensor) {
    const visibleLayers = layerFilterValue();
    const connectionLayers = (connections) => {
      const layers = new Set();
      for (const connection of connections || []) {
        for (const layerName of Object.keys(connection.by_layer || {})) {
          if (filterMatches(visibleLayers, layerName)) {
            layers.add(layerName);
          }
        }
      }
      return [...layers];
    };
    const productionLayers = connectionLayers(tensor.production_by_pe);
    const layers = productionLayers.length
      ? productionLayers
      : connectionLayers(tensor.consumption_by_pe);
    const firstLayer = [...layers].sort(
      (left, right) =>
        (layerOrderByName.get(left) ?? Number.MAX_SAFE_INTEGER) -
          (layerOrderByName.get(right) ?? Number.MAX_SAFE_INTEGER) ||
        left.localeCompare(right),
    )[0];
    return firstLayer || "Unassigned tensors";
  }

  function tensorRelationshipSource(tensor) {
    const layer = tensorRelationshipLayer(tensor);
    return {
      id: tensor.id,
      label: tensor.id,
      group: layer,
      order: [
        layerOrderByName.get(layer) ?? Number.MAX_SAFE_INTEGER,
        tensor.addr || "0",
      ],
    };
  }

  function sortTensorRelationshipSources(sources) {
    return sources.sort(
      (left, right) =>
        left.order[0] - right.order[0] ||
        bigIntCompare(left.order[1], right.order[1]) ||
        left.id.localeCompare(right.id),
    );
  }

  function tensorMemoryRelationshipModel() {
    const measure = relationshipControls.measure.value;
    const memories = relationshipMemories();
    const sources = [];
    const edges = [];
    for (const tensor of tensorsForContext()) {
      const tensorEdges = memoryRelationshipEdges(
        tensor.id,
        [tensor],
        memories,
        measure,
        layerFilterValue(),
        peFilterValue(),
      );
      if (tensorEdges.length) {
        sources.push(tensorRelationshipSource(tensor));
        edges.push(...tensorEdges);
      }
    }
    sortTensorRelationshipSources(sources);
    return {
      sources,
      targets: relationshipMemoryTargets(memories),
      edges,
      sourceLabel: "tensors",
      targetLabel: "memories",
    };
  }

  function tensorPeRelationshipModel() {
    const measure = relationshipControls.measure.value;
    const sources = [];
    const targetsById = new Map();
    const edges = [];
    for (const tensor of tensorsForContext()) {
      const traffic = tensorTrafficFor(tensor);
      const memoryShare = tensorMemoryShare(tensor);
      const connections = measure === "read" ? traffic.reads : traffic.writes;
      if (!connections.length || memoryShare === 0) {
        continue;
      }
      sources.push(tensorRelationshipSource(tensor));
      for (const connection of connections) {
        const pe = pesByName.get(connection.pe) || {};
        targetsById.set(connection.pe, {
          id: connection.pe,
          label: connection.pe,
          group: `PE row ${pe.row ?? 0}`,
          order: [Number(pe.row ?? 0), Number(pe.col ?? 0)],
        });
        edges.push({
          source: tensor.id,
          target: connection.pe,
          value: scaleTensorToMemory(tensor, connection.bytes),
        });
      }
    }
    const targets = [...targetsById.values()].sort(
      (left, right) =>
        left.order[0] - right.order[0] ||
        left.order[1] - right.order[1] ||
        left.id.localeCompare(right.id),
    );
    sortTensorRelationshipSources(sources);
    return {
      sources,
      targets,
      edges,
      sourceLabel: "tensors",
      targetLabel: "PEs",
    };
  }

  function layerBand(name) {
    const match = name.match(/\d+/);
    if (!match) {
      return name === "pre-layer" ? "Pre-layer" : "Unassigned layers";
    }
    const number = Number(match[0]);
    const start = Math.floor((number - 1) / 10) * 10 + 1;
    return `Layers ${start}-${start + 9}`;
  }

  function relationshipModel() {
    const key = cacheKey(
      relationshipControls.mode.value,
      relationshipControls.measure.value,
      layerFilterValue(),
      peFilterValue(),
      memoryFilterValue(),
      tensorFilterValue(),
    );
    if (relationshipModelCache.has(key)) {
      return relationshipModelCache.get(key);
    }
    const builders = {
      compute: computeRelationshipModel,
      memory: memoryRelationshipModel,
      "pe-memory": peMemoryRelationshipModel,
      "tensor-memory": tensorMemoryRelationshipModel,
      "tensor-pe": tensorPeRelationshipModel,
    };
    const model = builders[relationshipControls.mode.value]();
    relationshipModelCache.set(key, model);
    return model;
  }

  function positionArc(nodes, startAngle, endAngle, centerX, centerY, radius) {
    const denominator = Math.max(nodes.length - 1, 1);
    for (let index = 0; index < nodes.length; index++) {
      const angle =
        nodes.length === 1
          ? (startAngle + endAngle) / 2
          : startAngle + ((endAngle - startAngle) * index) / denominator;
      nodes[index].angle = angle;
      nodes[index].x = centerX + Math.cos(angle) * radius;
      nodes[index].y = centerY + Math.sin(angle) * radius;
    }
  }

  function relationshipGroupAnchors(nodes, centerX, centerY, radius) {
    const grouped = new Map();
    for (const node of nodes) {
      const group = grouped.get(node.group) || { name: node.group, nodes: [] };
      group.nodes.push(node);
      grouped.set(node.group, group);
    }
    for (const group of grouped.values()) {
      const angle =
        group.nodes.reduce((sum, node) => sum + node.angle, 0) /
        group.nodes.length;
      group.angle = angle;
      group.x = centerX + Math.cos(angle) * radius;
      group.y = centerY + Math.sin(angle) * radius;
    }
    return grouped;
  }

  function drawBundledCurve(context, hierarchyPoints, strength) {
    const start = hierarchyPoints[0];
    const end = hierarchyPoints[hierarchyPoints.length - 1];
    const points = hierarchyPoints.map((point, index) => {
      const progress = index / (hierarchyPoints.length - 1);
      const line = {
        x: start.x + (end.x - start.x) * progress,
        y: start.y + (end.y - start.y) * progress,
      };
      return {
        x: line.x + (point.x - line.x) * strength,
        y: line.y + (point.y - line.y) * strength,
      };
    });
    context.beginPath();
    context.moveTo(points[0].x, points[0].y);
    for (let index = 0; index < points.length - 1; index++) {
      const previous = points[Math.max(index - 1, 0)];
      const current = points[index];
      const next = points[index + 1];
      const following = points[Math.min(index + 2, points.length - 1)];
      context.bezierCurveTo(
        current.x + (next.x - previous.x) / 6,
        current.y + (next.y - previous.y) / 6,
        next.x - (following.x - current.x) / 6,
        next.y - (following.y - current.y) / 6,
        next.x,
        next.y,
      );
    }
    context.stroke();
  }

  function relationshipEdgeAlpha(edgeCount, weight) {
    const density = Math.min(edgeCount / 250, 1);
    const base = 0.28 - density * 0.18;
    const range = 0.5 - density * 0.25;
    return base + weight * range;
  }

  function svgNode(tag, attributes = {}) {
    const node = document.createElementNS("http://www.w3.org/2000/svg", tag);
    for (const [name, value] of Object.entries(attributes)) {
      node.setAttribute(name, value);
    }
    return node;
  }

  function relationshipEntityLabel(label, count) {
    if (count !== 1) {
      return label;
    }
    return (
      { layers: "layer", PEs: "PE", memories: "memory", tensors: "tensor" }[
        label
      ] || label
    );
  }

  function relationshipSelection() {
    if (relationshipControls.mode.value === "pe-memory") {
      return {
        source: state.selectedPe?.name,
        target: state.selectedMemoryName,
      };
    }
    if (relationshipControls.mode.value === "tensor-memory") {
      return {
        source: state.selectedTensor?.id,
        target: state.selectedMemoryName,
      };
    }
    if (relationshipControls.mode.value === "tensor-pe") {
      return {
        source: state.selectedTensor?.id,
        target: state.selectedPe?.name,
      };
    }
    return {
      source: state.selectedLayerName,
      target:
        relationshipControls.mode.value === "memory"
          ? state.selectedMemoryName
          : state.selectedPe?.name,
    };
  }

  function relationshipEntityKind(side) {
    const kinds = {
      compute: { source: "layer", target: "PE" },
      memory: { source: "layer", target: "memory" },
      "pe-memory": { source: "PE", target: "memory" },
      "tensor-memory": { source: "tensor", target: "memory" },
      "tensor-pe": { source: "tensor", target: "PE" },
    };
    return kinds[relationshipControls.mode.value][side];
  }

  function selectRelationshipEntity(side, id) {
    const kind = relationshipEntityKind(side);
    if (kind === "layer") {
      state.selectedLayerName = id;
    } else if (kind === "PE") {
      state.selectedPe = pesByName.get(id) || state.selectedPe;
    } else if (kind === "memory") {
      state.selectedMemoryName = id;
    } else if (kind === "tensor") {
      state.selectedTensor = tensorsById.get(id) || state.selectedTensor;
    }
    App.selectionChanged(kind.toLowerCase());
  }

  function filterRelationshipEntity(side, id) {
    const pickerName = {
      layer: "layers",
      PE: "pes",
      memory: "memories",
      tensor: "tensors",
    }[relationshipEntityKind(side)];
    selectOnlyFilterValue(filterPickers[pickerName], id);
  }

  function makeRelationshipEntityInteractive(
    element,
    node,
    side,
    keyboardAccessible = false,
  ) {
    let selectTimer = null;
    element.classList.add("interactive");
    if (keyboardAccessible) {
      element.setAttribute(
        "aria-label",
        `Select ${relationshipEntityKind(side)} ${node.label}`,
      );
      element.setAttribute("role", "button");
      element.setAttribute("tabindex", "0");
      element.setAttribute(
        "aria-pressed",
        (side === "source"
          ? relationshipSelection().source
          : relationshipSelection().target) === node.id
          ? "true"
          : "false",
      );
      element.addEventListener("keydown", (event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          selectRelationshipEntity(side, node.id);
        }
      });
    }
    element.addEventListener("click", () => {
      clearTimeout(selectTimer);
      selectTimer = setTimeout(
        () => selectRelationshipEntity(side, node.id),
        220,
      );
    });
    element.addEventListener("dblclick", (event) => {
      event.preventDefault();
      clearTimeout(selectTimer);
      filterRelationshipEntity(side, node.id);
    });
  }

  function renderRelationships() {
    if (relationshipBundle.closest("[data-view]")?.hidden) {
      return;
    }
    relationshipControls.strengthValue.value = `${relationshipControls.strength.value}%`;
    relationshipBundle.innerHTML = "";
    const requiresPlatform = ["memory", "pe-memory", "tensor-memory"].includes(
      relationshipControls.mode.value,
    );
    if (requiresPlatform && !(data.memory?.platform_memories || []).length) {
      relationshipBundle.innerHTML = `<p class="memory-empty">Provide a platform for memory relationships.</p>`;
      return;
    }

    const model = relationshipModel();
    if (!model.edges.length) {
      relationshipBundle.innerHTML = `<p class="memory-empty">No relationships match the current filters and measure.</p>`;
      return;
    }
    const sourceLabel = relationshipEntityLabel(
      model.sourceLabel,
      model.sources.length,
    );
    const targetLabel = relationshipEntityLabel(
      model.targetLabel,
      model.targets.length,
    );
    const width = 1000;
    const height = 620;
    const centerX = width / 2;
    const centerY = height / 2;
    const leafRadius = 250;
    const groupRadius = 132;
    positionArc(
      model.sources,
      Math.PI * 0.58,
      Math.PI * 1.42,
      centerX,
      centerY,
      leafRadius,
    );
    positionArc(
      model.targets,
      -Math.PI * 0.42,
      Math.PI * 0.42,
      centerX,
      centerY,
      leafRadius,
    );
    const sourceGroups = relationshipGroupAnchors(
      model.sources,
      centerX,
      centerY,
      groupRadius,
    );
    const targetGroups = relationshipGroupAnchors(
      model.targets,
      centerX,
      centerY,
      groupRadius,
    );
    const sourcesById = new Map(model.sources.map((node) => [node.id, node]));
    const targetsById = new Map(model.targets.map((node) => [node.id, node]));
    const maximum = model.edges.reduce(
      (max, edge) => bigIntMax(max, edge.value),
      1n,
    );
    const total = model.edges.reduce((sum, edge) => sum + edge.value, 0n);
    const sourceTotals = new Map();
    const targetTotals = new Map();
    for (const edge of model.edges) {
      sourceTotals.set(
        edge.source,
        toBigInt(sourceTotals.get(edge.source)) + edge.value,
      );
      targetTotals.set(
        edge.target,
        toBigInt(targetTotals.get(edge.target)) + edge.value,
      );
    }
    const maximumSourceTotal = [...sourceTotals.values()].reduce(
      (max, value) => bigIntMax(max, value),
      1n,
    );
    const maximumTargetTotal = [...targetTotals.values()].reduce(
      (max, value) => bigIntMax(max, value),
      1n,
    );
    const selection = relationshipSelection();
    const selectedSource = selection.source;
    const selectedTarget = selection.target;
    const strength = Number(relationshipControls.strength.value) / 100;

    const shell = document.createElement("div");
    shell.className = "relationship-plot";
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    canvas.setAttribute("aria-hidden", "true");
    const context = canvas.getContext("2d");
    const styles = getComputedStyle(document.documentElement);
    const mode = relationshipControls.measure.value;
    const edgeColor =
      mode === "read"
        ? styles.getPropertyValue("--read").trim()
        : mode === "write"
          ? styles.getPropertyValue("--write").trim()
          : styles.getPropertyValue("--activity-strong").trim();

    const edgePoints = (edge) => {
      const source = sourcesById.get(edge.source);
      const target = targetsById.get(edge.target);
      return [
        source,
        sourceGroups.get(source.group),
        { x: centerX - 28, y: centerY },
        { x: centerX + 28, y: centerY },
        targetGroups.get(target.group),
        target,
      ];
    };
    for (const edge of model.edges) {
      const weight = Math.sqrt(ratioPercent(edge.value, maximum) / 100);
      context.strokeStyle = edgeColor;
      context.globalAlpha = relationshipEdgeAlpha(model.edges.length, weight);
      context.lineWidth = 0.35 + weight * 1.8;
      drawBundledCurve(context, edgePoints(edge), strength);
    }
    context.globalAlpha = 1;

    const svg = svgNode("svg", {
      viewBox: `0 0 ${width} ${height}`,
      role: "group",
      "aria-label": `${model.edges.length} bundled relationships between ${model.sources.length} ${sourceLabel} and ${model.targets.length} ${targetLabel}`,
    });
    const title = svgNode("title");
    title.textContent = "Hierarchical edge bundle of timetable relationships";
    svg.append(title);
    const hierarchy = svgNode("g", { class: "relationship-hierarchy" });
    for (const [groups, nodes] of [
      [sourceGroups, model.sources],
      [targetGroups, model.targets],
    ]) {
      for (const node of nodes) {
        const group = groups.get(node.group);
        hierarchy.append(
          svgNode("line", { x1: node.x, y1: node.y, x2: group.x, y2: group.y }),
        );
      }
      for (const group of groups.values()) {
        hierarchy.append(
          svgNode("line", {
            x1: group.x,
            y1: group.y,
            x2: centerX,
            y2: centerY,
          }),
        );
      }
    }
    svg.append(hierarchy);

    const appendNodes = (nodes, side) => {
      const group = svgNode("g", { class: `relationship-nodes ${side}` });
      const labelStride =
        side === "source"
          ? Math.ceil(nodes.length / 28)
          : Math.ceil(nodes.length / 24);
      const totals = side === "source" ? sourceTotals : targetTotals;
      const maximumTotal =
        side === "source" ? maximumSourceTotal : maximumTargetTotal;
      nodes.forEach((node, index) => {
        const selected =
          side === "source"
            ? node.id === selectedSource
            : node.id === selectedTarget;
        const nodeTotal = totals.get(node.id) || 0n;
        const radius =
          2.5 + Math.sqrt(ratioPercent(nodeTotal, maximumTotal) / 100) * 5;
        const circle = svgNode("circle", {
          cx: node.x,
          cy: node.y,
          r: radius,
          class: `${mode} weighted${selected ? " selected" : ""}`,
        });
        const tooltip = svgNode("title");
        tooltip.textContent = `${node.label}: ${relationshipControls.mode.value === "compute" ? formatCount(nodeTotal) : formatBytes(nodeTotal)}; click to select, double-click to filter`;
        circle.append(tooltip);
        makeRelationshipEntityInteractive(circle, node, side, true);
        group.append(circle);
        const showLabel =
          selected || nodes.length <= 32 || index % labelStride === 0;
        if (showLabel) {
          const labelRadius = leafRadius + 12;
          const x = centerX + Math.cos(node.angle) * labelRadius;
          const y = centerY + Math.sin(node.angle) * labelRadius;
          const text = svgNode("text", {
            x,
            y,
            "text-anchor": x < centerX ? "end" : "start",
            class: selected ? "selected" : "",
          });
          text.textContent = node.label;
          makeRelationshipEntityInteractive(text, node, side);
          group.append(text);
        }
      });
      svg.append(group);
    };
    appendNodes(model.sources, "source");
    appendNodes(model.targets, "target");

    const groupLabels = svgNode("g", { class: "relationship-group-labels" });
    for (const group of [...sourceGroups.values(), ...targetGroups.values()]) {
      const text = svgNode("text", {
        x: group.x,
        y: group.y - 6,
        "text-anchor": "middle",
      });
      text.textContent = group.name;
      groupLabels.append(text);
    }
    svg.append(groupLabels);
    shell.append(canvas, svg);

    const measureLabel =
      relationshipControls.measure.options[
        relationshipControls.measure.selectedIndex
      ]?.textContent || "Value";
    const status = document.createElement("div");
    status.className = "relationship-status";
    status.innerHTML = `
    <span><i class="${mode}"></i>${escapeHtml(measureLabel)}</span>
    <span>${fmt.format(model.edges.length)} links</span>
    <span>${fmt.format(model.sources.length)} ${escapeHtml(sourceLabel)}</span>
    <span>${fmt.format(model.targets.length)} ${escapeHtml(targetLabel)}</span>
    <strong>${mode === "nodes" || relationshipControls.mode.value === "compute" ? formatCount(total) : formatBytes(total)} total</strong>
  `;
    relationshipBundle.append(shell, status);
  }

  Object.assign(App, {
    updateRelationshipMeasureOptions,
    renderRelationships,
  });
})();
