// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

(() => {
  const App = window.GWR_VISUALISATION_APP;
  const {
    data,
    state,
    computeNodes,
    tensorsById,
    timetableGraph,
    timetableGraphOverview,
    timetableGraphStatus,
    timetableGraphControls,
    selectedNodePanel,
    computeNodeTypes,
    machineOpTypes,
    layerFilterValue,
    peFilterValue,
    tensorFilterValue,
    memoryFilterValue,
    filterMatches,
    isAllFilter,
    filteredTensors,
    tensorTraffic,
    cacheKey,
    formatBytes,
    formatCount,
    toBigInt,
    escapeHtml,
    markEntityElement,
    setHoveredEntity,
    layoutTimetableGraph,
    timetableGraphNodeBounds,
    timetableGraphTensorNeedsBypass,
    timetableGraphTensorPlacement,
    timetableGraphTensorRole,
    timetableGraphAreaScaledRadius,
    timetableGraphGridGeometry,
    createCanvasViewport,
    createViewportTransform,
    pointerPosition,
    stableCategoryColours,
    stableDataTypeColours,
  } = App;

  const MAX_VISIBLE_NODES = 50_000;
  const MIN_TENSOR_HALF_HEIGHT = 32;
  const MAX_TENSOR_HALF_HEIGHT = 480;
  const TENSOR_HALF_WIDTH = 36;
  const TENSOR_FILL_OPACITY = 0.72;
  const MIN_COMPUTE_RADIUS = 8;
  const MAX_COMPUTE_RADIUS = 32;
  const MIN_GROUP_RADIUS = 10;
  const MAX_GROUP_RADIUS = 40;
  const EXPANDED_GROUP_PADDING = 10;
  const DEFAULT_EXPANDED_GROUP_MAX_ROWS = 32;
  const EXPANDED_GROUP_GAP = 9;
  const LAYER_RANK_STRIDE = 1_000;
  const LAYER_HEADER_HEIGHT = 28;
  const graphData = data.graph || {};
  const tensorList = data.tensors || [];
  const maximumTensorBytes = tensorList.reduce((maximum, tensor) => {
    const size = toBigInt(tensor.num_bytes);
    return size > maximum ? size : maximum;
  }, 0n);
  const maximumComputeMachineOps = computeNodes.reduce((maximum, compute) => {
    const total = toBigInt(compute.machine_ops);
    return total > maximum ? total : maximum;
  }, 0n);
  const computeById = new Map(computeNodes.map((node) => [node.id, node]));
  const computeIndexById = new Map(
    computeNodes.map((node, index) => [node.id, index]),
  );
  const layerNames = [...new Set(computeNodes.map((node) => node.layer))].sort(
    (left, right) =>
      layerNumber(left) - layerNumber(right) ||
      left.localeCompare(right, undefined, { numeric: true }),
  );
  const layerOrdinalByName = new Map(
    layerNames.map((layer, index) => [layer, index]),
  );
  const operationColours = new Map(
    computeNodeTypes.map((operation) => [operation.name, operation.colour]),
  );
  const computeGroups = (
    graphData.compute_groups?.length
      ? graphData.compute_groups
      : computeNodes.map((node, index) => ({ id: node.id, members: [index] }))
  ).map((group, index) => ({ ...group, index }));
  const maximumGroupNodeCount = Math.max(
    1,
    ...computeGroups.map((group) => group.members.length),
  );
  const maximumGroupMachineOps = computeGroups.reduce((maximum, group) => {
    const total = group.members.reduce(
      (sum, member) => sum + toBigInt(computeNodes[member]?.machine_ops),
      0n,
    );
    return total > maximum ? total : maximum;
  }, 0n);
  const groupByCompute = new Array(computeNodes.length);
  for (const group of computeGroups) {
    for (const member of group.members) {
      groupByCompute[member] = group;
    }
  }

  const expandedGroups = new Set();
  const expandedLayers = new Set(computeNodes.map((node) => node.layer));
  const actualEdges = [];
  const adjacency = new Map();
  let currentModel = null;
  let renderedModel = null;
  let modelDirty = true;
  let transform = createViewportTransform();
  let graphViewport = null;
  let drawFrame = null;
  let resizeObserver = null;
  const overviewBaseCanvas = document.createElement("canvas");
  let overviewBaseModel = null;
  let overviewBaseWidth = 0;
  let overviewBaseHeight = 0;
  let overviewBasePixelRatio = 0;
  let overviewTransform = createViewportTransform();
  let overviewPointerId = null;
  let overviewRangeSelection = null;
  let controlsBound = false;
  let hoveredNode = null;
  let colourCache = null;
  let pendingFocusSelection = null;
  let navigationDirection = "outputs";
  let navigationIndex = 0;
  let navigationSelectionKey = null;
  let navigationTargetSelection = null;
  let navigationCandidateSelections = [];
  let tensorColourMode = timetableGraphControls.tensorColour.value || "role";
  let groupSizeMode = timetableGraphControls.groupSize.value || "nodes";
  const nodeScales = { tensor: 1, compute: 1 };
  let computeGroupMaxRows = DEFAULT_EXPANDED_GROUP_MAX_ROWS;
  let computeLaneMode =
    timetableGraphControls.computeLanes.value || "operation";
  let edgeDisplayMode = "selection";
  let tensorTrafficCacheContext = null;
  const tensorTrafficCache = new Map();

  function actualKey(kind, id) {
    return `${kind}:${id}`;
  }

  function addAdjacency(key, edge) {
    if (!adjacency.has(key)) {
      adjacency.set(key, []);
    }
    adjacency.get(key).push(edge);
  }

  function addActualEdge(edge) {
    actualEdges.push(edge);
    addAdjacency(edge.sourceKey, edge);
    addAdjacency(edge.targetKey, edge);
  }

  for (const tensor of tensorList) {
    for (const [accessIndex, access] of (tensor.accesses || []).entries()) {
      const compute = computeNodes[access.node];
      if (!compute) {
        continue;
      }
      const tensorKey = actualKey("tensor", tensor.id);
      const computeKey = actualKey("compute", compute.id);
      const read = access.direction === "read";
      addActualEdge({
        sourceKey: read ? tensorKey : computeKey,
        targetKey: read ? computeKey : tensorKey,
        type: access.direction,
        tensor,
        compute,
        access,
        accessIndex,
      });
    }
  }
  for (const edge of graphData.control_edges || []) {
    const source = computeNodes[edge.from];
    const target = computeNodes[edge.to];
    if (source && target) {
      addActualEdge({
        sourceKey: actualKey("compute", source.id),
        targetKey: actualKey("compute", target.id),
        type: "control",
        source,
        target,
      });
    }
  }

  const tensorPlacement = new Map(
    tensorList.map((tensor) => [
      tensor.id,
      timetableGraphTensorPlacement(
        adjacency.get(actualKey("tensor", tensor.id)) || [],
      ),
    ]),
  );
  const tensorBypassSide = new Map();
  for (const tensor of tensorList) {
    if (
      !tensorPlacement.get(tensor.id) &&
      timetableGraphTensorNeedsBypass(
        adjacency.get(actualKey("tensor", tensor.id)) || [],
        (layer) => layerOrdinalByName.get(layer),
      )
    ) {
      tensorBypassSide.set(
        tensor.id,
        tensorBypassSide.size % 2 ? "bottom" : "top",
      );
    }
  }

  const tensorRoleDefinitions = [
    ["layer-input", "Layer input", "#0f766e"],
    ["intra-layer", "Intra-layer", "#7c3aed"],
    ["layer-transfer", "Layer transfer", "#64748b"],
    ["long-span", "Long-span transfer", "#c2416c"],
    ["graph-input", "Graph input", "#15803d"],
    ["graph-output", "Graph output", "#c2410c"],
    ["unconnected", "Unconnected", "#9ca3af"],
  ].map(([id, label, colour]) => ({ id, label, colour }));
  const tensorRoles = new Map(
    tensorList.map((tensor) => {
      const edges = adjacency.get(actualKey("tensor", tensor.id)) || [];
      return [
        tensor.id,
        timetableGraphTensorRole(
          edges,
          tensorPlacement.get(tensor.id),
          tensorBypassSide.has(tensor.id),
        ),
      ];
    }),
  );
  const dtypeColours = stableDataTypeColours(
    tensorList.map((tensor) => tensor.dtype),
  );
  const platformMemories = data.memory?.platform_memories || [];
  const memoryColours = stableCategoryColours(
    platformMemories.map((memory) => memory.name),
  );
  const tensorMemories = new Map(tensorList.map((tensor) => [tensor.id, []]));
  for (const memory of platformMemories) {
    for (const tensorId of memory.tensors || []) {
      tensorMemories.get(tensorId)?.push(memory.name);
    }
  }

  function layerNumber(layer) {
    const match = /(?:^|\s)(\d+)$/.exec(layer || "");
    return match ? Number(match[1]) : 0;
  }

  function layerLayoutRank(layer) {
    return (layerOrdinalByName.get(layer) || 0) * LAYER_RANK_STRIDE;
  }

  function tensorSizeRatio(tensor) {
    if (maximumTensorBytes === 0n) {
      return 0;
    }
    const scaled =
      (toBigInt(tensor.num_bytes) * 1_000_000n) / maximumTensorBytes;
    return Number(scaled) / 1_000_000;
  }

  function computeMachineOpsRatio(compute) {
    if (maximumComputeMachineOps === 0n) {
      return 0;
    }
    const scaled =
      (toBigInt(compute.machine_ops) * 1_000_000n) / maximumComputeMachineOps;
    return Number(scaled) / 1_000_000;
  }

  function computeRadius(compute) {
    const ratio = computeMachineOpsRatio(compute);
    return (
      nodeScales.compute *
      Math.sqrt(
        MIN_COMPUTE_RADIUS ** 2 +
          ratio * (MAX_COMPUTE_RADIUS ** 2 - MIN_COMPUTE_RADIUS ** 2),
      )
    );
  }

  function groupMachineOps(memberIndices) {
    return memberIndices.reduce(
      (total, index) => total + toBigInt(computeNodes[index]?.machine_ops),
      0n,
    );
  }

  function groupSizeRatio(memberIndices) {
    if (groupSizeMode === "machine-ops") {
      if (maximumGroupMachineOps === 0n) {
        return 0;
      }
      const scaled =
        (groupMachineOps(memberIndices) * 1_000_000n) / maximumGroupMachineOps;
      return Number(scaled) / 1_000_000;
    }
    return memberIndices.length / maximumGroupNodeCount;
  }

  function groupRadius(memberIndices) {
    return (
      nodeScales.compute *
      timetableGraphAreaScaledRadius(
        groupSizeRatio(memberIndices),
        MIN_GROUP_RADIUS,
        MAX_GROUP_RADIUS,
      )
    );
  }

  function computeLane(compute) {
    if (computeLaneMode === "operation") {
      return compute.op;
    }
    if (computeLaneMode === "machine-op") {
      return compute.dominant_machine_op || "no-machine-ops";
    }
    return null;
  }

  function groupLane(memberIndices) {
    const lanes = new Set(
      memberIndices.map((index) => computeLane(computeNodes[index])),
    );
    return lanes.size === 1 ? [...lanes][0] : "mixed-machine-ops";
  }

  function laneConfiguration(layoutItems) {
    const visible = new Set(
      layoutItems.map((node) => node.layoutLane).filter(Boolean),
    );
    if (!visible.size) {
      return { lanes: null, labels: new Map() };
    }
    const definitions =
      computeLaneMode === "machine-op" ? machineOpTypes : computeNodeTypes;
    const labels = new Map(
      definitions.map((definition) => [definition.name, definition.label]),
    );
    labels.set("no-machine-ops", "No machine ops");
    labels.set("mixed-machine-ops", "Mixed machine ops");
    const lanes = definitions
      .map((definition) => definition.name)
      .filter((name) => visible.has(name));
    for (const name of ["mixed-machine-ops", "no-machine-ops"]) {
      if (visible.has(name)) {
        lanes.push(name);
      }
    }
    return { lanes, labels };
  }

  function nodeContains(node, x, y) {
    if (node.shape !== "circle") {
      const bounds = timetableGraphNodeBounds(node);
      return (
        x >= bounds.left &&
        x <= bounds.right &&
        y >= bounds.top &&
        y <= bounds.bottom
      );
    }
    return Math.hypot(node.x - x, node.y - y) <= node.radius;
  }

  function isExpandedContainer(node) {
    return node.expanded && (node.kind === "layer" || node.kind === "group");
  }

  function distanceToNode(node, x, y) {
    if (node.shape !== "circle") {
      const bounds = timetableGraphNodeBounds(node);
      const dx = Math.max(bounds.left - x, x - bounds.right, 0);
      const dy = Math.max(bounds.top - y, y - bounds.bottom, 0);
      return Math.hypot(dx, dy);
    }
    return Math.max(Math.hypot(node.x - x, node.y - y) - node.radius, 0);
  }

  function distanceToOutline(node, x, y) {
    if (node.shape === "circle") {
      return Math.abs(Math.hypot(node.x - x, node.y - y) - node.radius);
    }
    const bounds = timetableGraphNodeBounds(node);
    if (!nodeContains(node, x, y)) {
      return distanceToNode(node, x, y);
    }
    return Math.min(
      x - bounds.left,
      bounds.right - x,
      y - bounds.top,
      bounds.bottom - y,
    );
  }

  function groupSelectionId(group) {
    return String(group.index);
  }

  function visualKey(selection) {
    if (!selection) {
      return null;
    }
    const prefix = { tensor: "t", compute: "c", group: "g", layer: "l" }[
      selection.kind
    ];
    return prefix ? `${prefix}:${selection.id}` : null;
  }

  function visibleContext() {
    const tensors = filteredTensors();
    const tensorIds = new Set(tensors.map((tensor) => tensor.id));
    const restrictByTensor =
      !isAllFilter(tensorFilterValue()) || !isAllFilter(memoryFilterValue());
    const connectedComputes = new Set();
    if (restrictByTensor) {
      for (const edge of actualEdges) {
        if (edge.tensor && tensorIds.has(edge.tensor.id)) {
          connectedComputes.add(edge.compute.id);
        }
      }
    }
    const computeIndices = new Set();
    for (const [index, node] of computeNodes.entries()) {
      if (
        filterMatches(layerFilterValue(), node.layer) &&
        filterMatches(peFilterValue(), node.pe) &&
        (!restrictByTensor || connectedComputes.has(node.id))
      ) {
        computeIndices.add(index);
      }
    }
    return { tensors, tensorIds, computeIndices };
  }

  function overLimitModel(context, count) {
    return {
      nodes: [],
      layoutItems: [],
      links: [],
      visualById: new Map(),
      nodeByActualKey: new Map(),
      context,
      overLimit: count,
    };
  }

  function interLayerTensorRank(tensor) {
    const connected = adjacency.get(actualKey("tensor", tensor.id)) || [];
    const writers = connected
      .filter((edge) => edge.type === "write")
      .map((edge) => layerOrdinalByName.get(edge.compute?.layer))
      .filter(Number.isFinite);
    if (writers.length) {
      return (Math.max(...writers) + 1) * LAYER_RANK_STRIDE - 1;
    }
    const readers = connected
      .filter((edge) => edge.type === "read")
      .map((edge) => layerOrdinalByName.get(edge.compute?.layer))
      .filter(Number.isFinite);
    return readers.length ? Math.min(...readers) * LAYER_RANK_STRIDE - 1 : -1;
  }

  function tensorVisualNode(tensor, layoutOrder) {
    const sizeRatio = tensorSizeRatio(tensor);
    const node = {
      id: `t:${tensor.id}`,
      kind: "tensor",
      shape: "rect",
      label: tensor.id,
      tensor,
      sizeRatio,
      bypassSide: tensorBypassSide.get(tensor.id),
      halfWidth: TENSOR_HALF_WIDTH * nodeScales.tensor,
      halfHeight:
        nodeScales.tensor *
        (MIN_TENSOR_HALF_HEIGHT +
          sizeRatio * (MAX_TENSOR_HALF_HEIGHT - MIN_TENSOR_HALF_HEIGHT)),
      layoutRank: interLayerTensorRank(tensor),
      layoutOrder,
    };
    node.topNode = node;
    node.layoutAnchor = node;
    return node;
  }

  function computeVisualNode(index, group, layerNode, layoutOrder) {
    const compute = computeNodes[index];
    const node = {
      id: `c:${compute.id}`,
      kind: "compute",
      shape: "circle",
      label: compute.id,
      group,
      compute,
      memberIndices: [index],
      radius: computeRadius(compute),
      layoutLane: computeLane(compute),
      layoutRank: layerNode.layoutRank,
      layoutOrder,
      topNode: layerNode,
    };
    node.layoutAnchor = node;
    return node;
  }

  function groupVisualNode(group, visibleMembers, layerNode, layoutOrder) {
    if (group.members.length === 1) {
      return computeVisualNode(
        visibleMembers[0],
        group,
        layerNode,
        layoutOrder,
      );
    }
    const expanded = expandedGroups.has(group.index);
    const children = expanded
      ? visibleMembers.map((index, childIndex) =>
          computeVisualNode(index, group, layerNode, childIndex),
        )
      : [];
    const grid = expanded
      ? timetableGraphGridGeometry(
          children,
          computeGroupMaxRows,
          EXPANDED_GROUP_GAP,
          EXPANDED_GROUP_PADDING,
        )
      : null;
    const node = {
      id: `g:${group.index}`,
      kind: "group",
      shape: expanded ? "rect" : "circle",
      label: group.id,
      group,
      compute: computeNodes[visibleMembers[0]],
      memberIndices: visibleMembers,
      expanded,
      children,
      grid,
      radius: expanded ? undefined : groupRadius(visibleMembers),
      halfWidth: expanded ? grid.width / 2 : undefined,
      halfHeight: expanded ? grid.height / 2 : undefined,
      layoutRank: layerNode.layoutRank,
      layoutOrder,
      layoutLane: groupLane(visibleMembers),
      topNode: layerNode,
    };
    node.layoutAnchor = node;
    for (const child of children) {
      child.parentGroup = node;
      child.layoutAnchor = node;
    }
    return node;
  }

  function buildVisibleModel() {
    const context = visibleContext();
    const nodes = [];
    const layoutItems = [];
    const nodeByActualKey = new Map();
    const visualById = new Map();
    const layerTensors = new Map();
    const visibleLayers = new Set(
      [...context.computeIndices]
        .map((index) => computeNodes[index]?.layer)
        .filter(Boolean),
    );

    for (const [index, tensor] of context.tensors.entries()) {
      const node = tensorVisualNode(tensor, index);
      const placement = tensorPlacement.get(tensor.id);
      if (placement && visibleLayers.has(placement.layer)) {
        if (!layerTensors.has(placement.layer)) {
          layerTensors.set(placement.layer, []);
        }
        node.layerInput = placement.role === "input";
        layerTensors.get(placement.layer).push(node);
      } else {
        nodes.push(node);
        layoutItems.push(node);
        visualById.set(node.id, node);
        nodeByActualKey.set(actualKey("tensor", tensor.id), node);
      }
    }

    const groupsByLayer = new Map();
    for (const group of computeGroups) {
      const visibleMembers = group.members.filter((index) =>
        context.computeIndices.has(index),
      );
      if (!visibleMembers.length) {
        continue;
      }
      group.visibleMembers = visibleMembers;
      const layer = computeNodes[visibleMembers[0]].layer;
      if (!groupsByLayer.has(layer)) {
        groupsByLayer.set(layer, []);
      }
      groupsByLayer.get(layer).push({ group, visibleMembers });
    }

    let projectedNodeCount = nodes.length;
    for (const [layer, layerGroups] of groupsByLayer) {
      projectedNodeCount += 1;
      if (!expandedLayers.has(layer)) {
        continue;
      }
      projectedNodeCount += layerTensors.get(layer)?.length || 0;
      for (const { group, visibleMembers } of layerGroups) {
        projectedNodeCount +=
          group.members.length > 1 && expandedGroups.has(group.index)
            ? visibleMembers.length + 1
            : 1;
      }
    }
    if (projectedNodeCount > MAX_VISIBLE_NODES) {
      return overLimitModel(context, projectedNodeCount);
    }

    for (const [layer, layerGroups] of groupsByLayer) {
      const id = `l:${layer}`;
      const expanded = expandedLayers.has(layer);
      const computeCount = layerGroups.reduce(
        (total, item) => total + item.visibleMembers.length,
        0,
      );
      const layerNode = {
        id,
        kind: "layer",
        label: layer,
        layer,
        layerGroups,
        layerTensorCount: layerTensors.get(layer)?.length || 0,
        memberIndices: layerGroups.flatMap((item) => item.visibleMembers),
        expanded,
        shape: "rect",
        halfWidth: expanded ? 50 : 42,
        halfHeight: expanded ? 42 : Math.min(22 + Math.sqrt(computeCount), 42),
        layoutRank: layerLayoutRank(layer),
        layoutOrder: layerOrdinalByName.get(layer) || 0,
      };
      layerNode.topNode = layerNode;
      nodes.push(layerNode);
      visualById.set(id, layerNode);

      if (!expanded) {
        layerNode.layoutAnchor = layerNode;
        layoutItems.push(layerNode);
        for (const index of layerNode.memberIndices) {
          nodeByActualKey.set(
            actualKey("compute", computeNodes[index].id),
            layerNode,
          );
        }
        for (const tensor of layerTensors.get(layer) || []) {
          nodeByActualKey.set(actualKey("tensor", tensor.tensor.id), layerNode);
        }
        continue;
      }

      const computeChildren = layerGroups
        .sort((left, right) =>
          left.group.id.localeCompare(right.group.id, undefined, {
            numeric: true,
          }),
        )
        .map(({ group, visibleMembers }, index) =>
          groupVisualNode(group, visibleMembers, layerNode, index),
        );
      const tensorChildren = layerTensors.get(layer) || [];
      const layerInputs = tensorChildren.filter((tensor) => tensor.layerInput);
      layerInputs.forEach((tensor, index) => {
        tensor.layerInputSide = index % 2 ? "bottom" : "top";
      });
      for (const tensor of tensorChildren) {
        tensor.topNode = layerNode;
      }
      const layerChildren = [...computeChildren, ...tensorChildren];
      layerNode.layoutChildren = layerChildren;
      for (const node of layerChildren) {
        nodes.push(node);
        layoutItems.push(node);
        visualById.set(node.id, node);
        if (node.kind === "tensor") {
          nodeByActualKey.set(actualKey("tensor", node.tensor.id), node);
          continue;
        }
        for (const child of node.children || []) {
          nodes.push(child);
          visualById.set(child.id, child);
          nodeByActualKey.set(actualKey("compute", child.compute.id), child);
        }
        if (node.kind === "group" && !node.expanded) {
          for (const index of node.memberIndices) {
            nodeByActualKey.set(
              actualKey("compute", computeNodes[index].id),
              node,
            );
          }
        } else if (node.kind === "compute") {
          nodeByActualKey.set(actualKey("compute", node.compute.id), node);
        }
      }
    }

    if (nodes.length > MAX_VISIBLE_NODES) {
      return overLimitModel(context, nodes.length);
    }

    const linksByKey = new Map();
    for (const edge of actualEdges) {
      const source = nodeByActualKey.get(edge.sourceKey);
      const target = nodeByActualKey.get(edge.targetKey);
      if (!source || !target || source === target) {
        continue;
      }
      const key = `${source.id}|${target.id}|${edge.type}`;
      const existing = linksByKey.get(key);
      if (existing) {
        existing.count += 1;
        existing.edges.push(edge);
      } else {
        linksByKey.set(key, {
          source,
          target,
          type: edge.type,
          count: 1,
          edges: [edge],
        });
      }
    }
    const laneConfig = laneConfiguration(layoutItems);
    const model = {
      nodes,
      layoutItems,
      links: [...linksByKey.values()],
      visualById,
      nodeByActualKey,
      context,
      computeLanes: laneConfig.lanes,
      laneLabels: laneConfig.labels,
    };
    return layoutTimetableGraph(model);
  }

  function resolvedColour(value) {
    const sample = document.createElement("span");
    sample.style.color = value;
    sample.hidden = true;
    document.body.append(sample);
    const colour = getComputedStyle(sample).color;
    sample.remove();
    return colour;
  }

  function graphColours() {
    colourCache ||= {
      foreground: resolvedColour("var(--fg)"),
      muted: resolvedColour("var(--muted)"),
      panel: resolvedColour("var(--panel)"),
      border: resolvedColour("var(--border)"),
      selected: resolvedColour("var(--activity-strong)"),
      read: resolvedColour("var(--read)"),
      write: resolvedColour("var(--write)"),
      tensorUniform: resolvedColour("var(--muted)"),
    };
    return colourCache;
  }

  function canvasAndContext() {
    const canvas = timetableGraph.querySelector("canvas");
    const pixelRatio = window.devicePixelRatio || 1;
    const width = Math.max(timetableGraph.clientWidth, 320);
    const height = Math.max(timetableGraph.clientHeight, 420);
    const targetWidth = Math.round(width * pixelRatio);
    const targetHeight = Math.round(height * pixelRatio);
    if (canvas.width !== targetWidth || canvas.height !== targetHeight) {
      canvas.width = targetWidth;
      canvas.height = targetHeight;
      canvas.style.width = `${width}px`;
      canvas.style.height = `${height}px`;
    }
    const context = canvas.getContext("2d");
    context.setTransform(pixelRatio, 0, 0, pixelRatio, 0, 0);
    return { canvas, context, width, height, pixelRatio };
  }

  function selectedVisualNode(node) {
    const selected = state.selectedNode;
    if (!selected) {
      return false;
    }
    if (node.kind === selected.kind && node.id === visualKey(selected)) {
      return true;
    }
    if (selected.kind === "compute" && node.kind === "group") {
      const index = computeIndexById.get(selected.id);
      return node.group.members.includes(index);
    }
    if (node.kind === "layer" && !node.expanded) {
      if (selected.kind === "compute") {
        return computeById.get(selected.id)?.layer === node.layer;
      }
      if (selected.kind === "group") {
        const group = computeGroups[Number(selected.id)];
        return computeNodes[group?.members[0]]?.layer === node.layer;
      }
      if (selected.kind === "tensor") {
        return tensorPlacement.get(selected.id)?.layer === node.layer;
      }
    }
    return false;
  }

  function requestDraw() {
    if (drawFrame !== null) {
      return;
    }
    drawFrame = window.requestAnimationFrame(() => {
      drawFrame = null;
      drawGraph();
      drawGraphOverview();
    });
  }

  function drawArrow(context, link, colours, emphasized = false) {
    const { start, control1, control2, end } = link.route;
    context.strokeStyle =
      link.type === "read"
        ? colours.read
        : link.type === "write"
          ? colours.write
          : colours.muted;
    const weight = Math.min(1 + Math.log2(link.count) * 0.35, 4);
    context.lineWidth = emphasized ? Math.max(weight, 4 / transform.k) : weight;
    context.setLineDash(link.type === "control" ? [5, 4] : []);
    context.beginPath();
    context.moveTo(start.x, start.y);
    context.bezierCurveTo(
      control1.x,
      control1.y,
      control2.x,
      control2.y,
      end.x,
      end.y,
    );
    context.stroke();
    context.setLineDash([]);

    const angle = Math.atan2(end.y - control2.y, end.x - control2.x);
    context.fillStyle = context.strokeStyle;
    context.beginPath();
    context.moveTo(end.x, end.y);
    context.lineTo(
      end.x - Math.cos(angle - Math.PI / 6) * 6,
      end.y - Math.sin(angle - Math.PI / 6) * 6,
    );
    context.lineTo(
      end.x - Math.cos(angle + Math.PI / 6) * 6,
      end.y - Math.sin(angle + Math.PI / 6) * 6,
    );
    context.closePath();
    context.fill();
  }

  function traceNodeShape(context, node, padding = 0) {
    context.beginPath();
    if (node.shape !== "circle") {
      const bounds = timetableGraphNodeBounds(node, padding);
      const width = bounds.right - bounds.left;
      const height = bounds.bottom - bounds.top;
      const preferredRadius =
        node.kind === "layer" ? 10 : node.kind === "group" ? 8 : 6;
      const radius = Math.min(preferredRadius + padding, width / 2, height / 2);
      context.moveTo(bounds.left + radius, bounds.top);
      context.lineTo(bounds.right - radius, bounds.top);
      context.quadraticCurveTo(
        bounds.right,
        bounds.top,
        bounds.right,
        bounds.top + radius,
      );
      context.lineTo(bounds.right, bounds.bottom - radius);
      context.quadraticCurveTo(
        bounds.right,
        bounds.bottom,
        bounds.right - radius,
        bounds.bottom,
      );
      context.lineTo(bounds.left + radius, bounds.bottom);
      context.quadraticCurveTo(
        bounds.left,
        bounds.bottom,
        bounds.left,
        bounds.bottom - radius,
      );
      context.lineTo(bounds.left, bounds.top + radius);
      context.quadraticCurveTo(
        bounds.left,
        bounds.top,
        bounds.left + radius,
        bounds.top,
      );
      context.closePath();
    } else {
      context.arc(node.x, node.y, node.radius + padding, 0, Math.PI * 2);
    }
  }

  function fillTensorRect(
    context,
    node,
    colour,
    opacity = TENSOR_FILL_OPACITY,
  ) {
    context.save();
    context.fillStyle = colour;
    context.globalAlpha = opacity;
    traceNodeShape(context, node);
    context.fill();
    context.restore();
  }

  function drawRepeatedTraffic(context, bounds, top, height, colour) {
    context.save();
    context.beginPath();
    context.rect(bounds.left, top, bounds.right - bounds.left, height);
    context.clip();
    context.strokeStyle = colour;
    context.globalAlpha = 0.9;
    context.lineWidth = 1.5 / transform.k;
    const span = height * 2 + bounds.right - bounds.left;
    const step = Math.max(7 / transform.k, span / 64);
    for (let x = bounds.left - height; x < bounds.right + height; x += step) {
      context.beginPath();
      context.moveTo(x, top + height);
      context.lineTo(x + height, top);
      context.stroke();
    }
    context.restore();
  }

  function cachedTensorTraffic(tensor) {
    const context = `${cacheKey(layerFilterValue())}|${cacheKey(peFilterValue())}`;
    if (context !== tensorTrafficCacheContext) {
      tensorTrafficCacheContext = context;
      tensorTrafficCache.clear();
    }
    if (!tensorTrafficCache.has(tensor.id)) {
      tensorTrafficCache.set(tensor.id, tensorTraffic(tensor));
    }
    return tensorTrafficCache.get(tensor.id);
  }

  function drawTensorTraffic(context, node, colours) {
    const bounds = timetableGraphNodeBounds(node);
    const halfHeight = (bounds.bottom - bounds.top) / 2;
    const traffic = cachedTensorTraffic(node.tensor);
    fillTensorRect(context, node, colours.panel, 0.55);
    context.save();
    traceNodeShape(context, node);
    context.clip();
    for (const [index, ratio, colour] of [
      [0, traffic.readRatio, colours.read],
      [1, traffic.writeRatio, colours.write],
    ]) {
      const top = bounds.top + index * halfHeight;
      if (ratio > 0) {
        context.save();
        context.fillStyle = colour;
        context.globalAlpha = 0.18 + Math.min(ratio, 1) * 0.72;
        context.fillRect(
          bounds.left,
          top,
          bounds.right - bounds.left,
          halfHeight,
        );
        context.restore();
      }
      if (ratio > 1) {
        drawRepeatedTraffic(context, bounds, top, halfHeight, colour);
      }
    }
    context.restore();
  }

  function drawTensorMemory(context, node, colours) {
    const memories = tensorMemories.get(node.tensor.id) || [];
    if (!memories.length) {
      fillTensorRect(context, node, colours.tensorUniform, 0.28);
      return;
    }
    const bounds = timetableGraphNodeBounds(node);
    const width = (bounds.right - bounds.left) / memories.length;
    context.save();
    traceNodeShape(context, node);
    context.clip();
    context.globalAlpha = TENSOR_FILL_OPACITY;
    memories.forEach((memory, index) => {
      context.fillStyle = memoryColours.get(memory);
      context.fillRect(
        bounds.left + width * index,
        bounds.top,
        width + 0.5,
        bounds.bottom - bounds.top,
      );
    });
    context.restore();
  }

  function drawTensorFill(context, node, colours) {
    if (tensorColourMode === "traffic") {
      drawTensorTraffic(context, node, colours);
      return;
    }
    if (tensorColourMode === "memory") {
      drawTensorMemory(context, node, colours);
      return;
    }
    const colour =
      tensorColourMode === "dtype"
        ? dtypeColours.get(node.tensor.dtype)
        : tensorColourMode === "uniform"
          ? colours.tensorUniform
          : tensorRoleDefinitions.find(
              (definition) => definition.id === tensorRoles.get(node.tensor.id),
            )?.colour;
    fillTensorRect(context, node, colour || colours.tensorUniform);
  }

  function drawNodeShape(context, node, colours) {
    const hovered = hoveredNode?.id === node.id;
    const container =
      (node.kind === "layer" && node.expanded) ||
      (node.kind === "group" && node.expanded);
    context.fillStyle =
      node.kind === "layer"
        ? colours.panel
        : operationColours.get(node.compute?.op) || colours.muted;
    context.strokeStyle = colours.border;
    context.lineWidth = (hovered ? 2 : 1) / transform.k;
    traceNodeShape(context, node);
    if (node.kind === "tensor") {
      drawTensorFill(context, node, colours);
    } else if (container) {
      context.save();
      context.globalAlpha = node.kind === "layer" ? 0.36 : 0.12;
      context.fill();
      context.restore();
      if (node.kind === "layer") {
        const bounds = timetableGraphNodeBounds(node);
        context.save();
        traceNodeShape(context, node);
        context.clip();
        context.fillStyle = colours.border;
        context.globalAlpha = 0.16;
        context.fillRect(
          bounds.left,
          bounds.top,
          bounds.right - bounds.left,
          Math.min(LAYER_HEADER_HEIGHT, bounds.bottom - bounds.top),
        );
        context.restore();
      }
    } else {
      context.fill();
    }
    traceNodeShape(context, node);
    context.stroke();
  }

  function drawSelectionHalo(context, node, colours) {
    const padding = 3 / transform.k;
    traceNodeShape(context, node, padding);
    context.strokeStyle = colours.panel;
    context.lineWidth = 7 / transform.k;
    context.stroke();
    traceNodeShape(context, node, padding);
    context.strokeStyle = colours.selected;
    context.lineWidth = 3 / transform.k;
    context.stroke();
  }

  function drawNavigationHalo(context, node, colours, highlighted) {
    traceNodeShape(context, node, 7 / transform.k);
    context.strokeStyle = highlighted ? colours.selected : colours.muted;
    context.lineWidth = (highlighted ? 3 : 1.5) / transform.k;
    context.setLineDash([5 / transform.k, 4 / transform.k]);
    if (!highlighted) {
      context.globalAlpha = 0.55;
    }
    context.stroke();
    context.globalAlpha = 1;
    context.setLineDash([]);
  }

  function drawNodeLabel(context, node, colours) {
    const selected = selectedVisualNode(node);
    const hovered = hoveredNode?.id === node.id;
    if (node.kind === "layer" && node.expanded) {
      if (transform.k < 0.4) {
        return;
      }
      const bounds = timetableGraphNodeBounds(node);
      const inset = 8 / transform.k;
      const fontSize = 11 / transform.k;
      const visibleLeft = transform.invert([0, 0])[0];
      const visibleRight = transform.invert([timetableGraph.clientWidth, 0])[0];
      const titleX = Math.max(bounds.left + inset, visibleLeft + inset);
      const summaryX = Math.min(bounds.right - inset, visibleRight - inset);
      context.font = `500 ${fontSize}px ui-sans-serif, system-ui, sans-serif`;
      context.textBaseline = "middle";
      context.fillStyle = colours.foreground;
      context.textAlign = "left";
      context.fillText(
        node.label,
        titleX,
        bounds.top + LAYER_HEADER_HEIGHT / 2,
      );
      const summary = `${node.memberIndices.length.toLocaleString()} computes · ${formatCount(groupMachineOps(node.memberIndices))} ops`;
      const availableWidth = summaryX - titleX;
      const requiredWidth =
        context.measureText(node.label).width +
        context.measureText(summary).width +
        20 / transform.k;
      if (availableWidth >= requiredWidth) {
        context.fillStyle = colours.muted;
        context.textAlign = "right";
        context.fillText(
          summary,
          summaryX,
          bounds.top + LAYER_HEADER_HEIGHT / 2,
        );
      }
      return;
    }
    if (node.kind === "layer" || selected || hovered) {
      context.fillStyle = colours.foreground;
      const screenScaled = selected || hovered;
      const fontSize = screenScaled ? 12 / transform.k : 12;
      const labelGap = screenScaled ? 7 / transform.k : 5;
      context.font = `${fontSize}px ui-sans-serif, system-ui, sans-serif`;
      context.textAlign = "center";
      context.textBaseline = "bottom";
      const label =
        node.label.length > 54 ? `${node.label.slice(0, 51)}…` : node.label;
      const bounds = timetableGraphNodeBounds(node);
      context.fillText(label, node.x, bounds.top - labelGap);
    }
  }

  function boundsIntersect(left, right) {
    return !(
      left.right < right.left ||
      left.left > right.right ||
      left.bottom < right.top ||
      left.top > right.bottom
    );
  }

  function viewportGraphBounds(width, height) {
    const [left, top] = transform.invert([0, 0]);
    const [right, bottom] = transform.invert([width, height]);
    const padding = 16 / transform.k;
    return {
      left: left - padding,
      right: right + padding,
      top: top - padding,
      bottom: bottom + padding,
    };
  }

  function drawGraph() {
    if (!currentModel || timetableGraph.closest("[data-view]")?.hidden) {
      return;
    }
    const { context, width, height } = canvasAndContext();
    const colours = graphColours();
    context.clearRect(0, 0, width, height);
    context.save();
    context.translate(transform.x, transform.y);
    context.scale(transform.k, transform.k);
    const viewport = viewportGraphBounds(width, height);
    const visibleNodes = currentModel.nodes.filter((node) =>
      boundsIntersect(timetableGraphNodeBounds(node), viewport),
    );
    const visibleLinks = currentModel.links.filter((link) =>
      boundsIntersect(link.bounds, viewport),
    );
    const backgroundNodes = visibleNodes.filter(isExpandedContainer);
    const backgroundNodeIds = new Set(backgroundNodes.map((node) => node.id));
    for (const node of backgroundNodes) {
      drawNodeShape(context, node, colours);
    }
    drawComputeLaneGuides(context, viewport, colours);
    for (const link of visibleLinks) {
      context.save();
      context.globalAlpha = edgeDisplayMode === "all" ? 0.5 : 0.28;
      drawArrow(context, link, colours);
      context.restore();
    }
    for (const node of visibleNodes.filter(
      (candidate) => !backgroundNodeIds.has(candidate.id),
    )) {
      drawNodeShape(context, node, colours);
    }
    for (const link of visibleLinks.filter((candidate) => {
      const selected =
        selectedVisualNode(candidate.source) ||
        selectedVisualNode(candidate.target);
      const hovered =
        hoveredNode?.id === candidate.source.id ||
        hoveredNode?.id === candidate.target.id;
      return selected || hovered;
    })) {
      context.save();
      context.globalAlpha = 1;
      drawArrow(context, link, colours, true);
      context.restore();
    }
    for (const node of visibleNodes.filter(selectedVisualNode)) {
      drawSelectionHalo(context, node, colours);
    }
    const seenNavigationNodes = new Set();
    const navigationNodes = navigationCandidateSelections
      .map((selection) => visualNodeForSelection(selection))
      .filter((node) => {
        if (!node || seenNavigationNodes.has(node)) {
          return false;
        }
        seenNavigationNodes.add(node);
        return true;
      });
    const navigationNode = visualNodeForSelection(navigationTargetSelection);
    for (const node of navigationNodes) {
      if (node !== navigationNode && visibleNodes.includes(node)) {
        drawNavigationHalo(context, node, colours, false);
      }
    }
    if (navigationNode && visibleNodes.includes(navigationNode)) {
      drawNavigationHalo(context, navigationNode, colours, true);
    }
    for (const node of visibleNodes) {
      drawNodeLabel(context, node, colours);
    }
    context.restore();
  }

  function drawComputeLaneGuides(context, viewport, colours) {
    if (!currentModel.lanes?.length) {
      return;
    }
    const left = Math.max(currentModel.bounds.left, viewport.left);
    const right = Math.min(currentModel.bounds.right, viewport.right);
    const visibleLeft = transform.invert([0, 0])[0];
    const labelLeft =
      Math.max(currentModel.bounds.left, visibleLeft) + 8 / transform.k;
    context.save();
    for (const lane of currentModel.lanes) {
      if (lane.bottom < viewport.top || lane.top > viewport.bottom) {
        continue;
      }
      context.fillStyle = colours.panel;
      context.globalAlpha = 0.2;
      context.fillRect(left, lane.top, right - left, lane.bottom - lane.top);
      context.globalAlpha = 0.42;
      context.strokeStyle = colours.border;
      context.lineWidth = 1 / transform.k;
      context.beginPath();
      context.moveTo(left, lane.centre);
      context.lineTo(right, lane.centre);
      context.stroke();
      if (transform.k >= 0.18) {
        const fontSize = 11 / transform.k;
        const horizontalPadding = 4 / transform.k;
        const verticalPadding = 2 / transform.k;
        context.font = `${fontSize}px ui-sans-serif, system-ui, sans-serif`;
        context.textAlign = "left";
        context.textBaseline = "middle";
        const label = currentModel.laneLabels?.get(lane.id) || lane.id;
        const textWidth = context.measureText(label).width;
        context.globalAlpha = 0.86;
        context.fillStyle = colours.panel;
        context.fillRect(
          labelLeft - horizontalPadding,
          lane.centre - fontSize / 2 - verticalPadding,
          textWidth + horizontalPadding * 2,
          fontSize + verticalPadding * 2,
        );
        context.globalAlpha = 0.9;
        context.fillStyle = colours.foreground;
        context.fillText(label, labelLeft, lane.centre);
      }
    }
    context.restore();
  }

  function overviewCanvasAndContext() {
    const canvas = timetableGraphOverview.querySelector("canvas");
    const pixelRatio = window.devicePixelRatio || 1;
    const width = Math.max(timetableGraphOverview.clientWidth, 1);
    const height = Math.max(timetableGraphOverview.clientHeight, 1);
    const targetWidth = Math.round(width * pixelRatio);
    const targetHeight = Math.round(height * pixelRatio);
    if (canvas.width !== targetWidth || canvas.height !== targetHeight) {
      canvas.width = targetWidth;
      canvas.height = targetHeight;
      canvas.style.width = `${width}px`;
      canvas.style.height = `${height}px`;
    }
    const context = canvas.getContext("2d");
    context.setTransform(pixelRatio, 0, 0, pixelRatio, 0, 0);
    return { canvas, context, width, height, pixelRatio };
  }

  function fittedOverviewTransform(width, height) {
    const bounds = currentModel.bounds;
    const padding = 12;
    const graphWidth = Math.max(bounds.right - bounds.left, 1);
    const graphHeight = Math.max(bounds.bottom - bounds.top, 1);
    const scale = Math.min(
      Math.max(width - padding * 2, 1) / graphWidth,
      Math.max(height - padding * 2, 1) / graphHeight,
    );
    return createViewportTransform(
      width / 2 - ((bounds.left + bounds.right) / 2) * scale,
      height / 2 - ((bounds.top + bounds.bottom) / 2) * scale,
      scale,
    );
  }

  function drawOverviewLink(context, link, colours) {
    const { start, control1, control2, end } = link.route;
    context.strokeStyle =
      link.type === "read"
        ? colours.read
        : link.type === "write"
          ? colours.write
          : colours.muted;
    context.globalAlpha = link.type === "control" ? 0.38 : 0.5;
    context.lineWidth = Math.max(0.75 / overviewTransform.k, 0.5);
    context.setLineDash(
      link.type === "control"
        ? [3 / overviewTransform.k, 3 / overviewTransform.k]
        : [],
    );
    context.beginPath();
    context.moveTo(start.x, start.y);
    context.bezierCurveTo(
      control1.x,
      control1.y,
      control2.x,
      control2.y,
      end.x,
      end.y,
    );
    context.stroke();
    context.setLineDash([]);
    context.globalAlpha = 1;
  }

  function drawOverviewNode(context, node, colours) {
    const container = isExpandedContainer(node);
    context.fillStyle =
      node.kind === "layer"
        ? colours.panel
        : operationColours.get(node.compute?.op) || colours.muted;
    if (node.kind === "tensor") {
      drawOverviewTensor(context, node, colours);
    } else {
      context.save();
      context.globalAlpha = container ? 0.3 : 0.8;
      traceNodeShape(context, node);
      context.fill();
      context.restore();
    }
    traceNodeShape(context, node);
    context.strokeStyle = colours.border;
    context.lineWidth = Math.max(0.75 / overviewTransform.k, 0.5);
    context.stroke();
  }

  function drawOverviewTensor(context, node, colours) {
    if (tensorColourMode === "memory") {
      drawTensorMemory(context, node, colours);
      return;
    }
    if (tensorColourMode === "traffic") {
      const bounds = timetableGraphNodeBounds(node);
      const halfHeight = (bounds.bottom - bounds.top) / 2;
      const traffic = cachedTensorTraffic(node.tensor);
      fillTensorRect(context, node, colours.panel, 0.45);
      for (const [index, ratio, colour] of [
        [0, traffic.readRatio, colours.read],
        [1, traffic.writeRatio, colours.write],
      ]) {
        if (ratio <= 0) {
          continue;
        }
        context.save();
        context.fillStyle = colour;
        context.globalAlpha = 0.25 + Math.min(ratio, 1) * 0.65;
        context.fillRect(
          bounds.left,
          bounds.top + index * halfHeight,
          bounds.right - bounds.left,
          halfHeight,
        );
        context.restore();
      }
      return;
    }
    const colour =
      tensorColourMode === "dtype"
        ? dtypeColours.get(node.tensor.dtype)
        : tensorColourMode === "uniform"
          ? colours.tensorUniform
          : tensorRoleDefinitions.find(
              (definition) => definition.id === tensorRoles.get(node.tensor.id),
            )?.colour;
    fillTensorRect(context, node, colour || colours.tensorUniform);
  }

  function rebuildOverviewBase(width, height, pixelRatio) {
    overviewBaseCanvas.width = Math.round(width * pixelRatio);
    overviewBaseCanvas.height = Math.round(height * pixelRatio);
    const context = overviewBaseCanvas.getContext("2d");
    context.setTransform(pixelRatio, 0, 0, pixelRatio, 0, 0);
    context.clearRect(0, 0, width, height);
    overviewTransform = fittedOverviewTransform(width, height);
    context.save();
    context.translate(overviewTransform.x, overviewTransform.y);
    context.scale(overviewTransform.k, overviewTransform.k);
    const colours = graphColours();
    const backgroundNodes = currentModel.nodes.filter(isExpandedContainer);
    const backgroundNodeIds = new Set(backgroundNodes.map((node) => node.id));
    for (const node of backgroundNodes) {
      drawOverviewNode(context, node, colours);
    }
    for (const link of currentModel.links) {
      drawOverviewLink(context, link, colours);
    }
    for (const node of currentModel.nodes.filter(
      (candidate) => !backgroundNodeIds.has(candidate.id),
    )) {
      drawOverviewNode(context, node, colours);
    }
    context.restore();
    overviewBaseModel = currentModel;
    overviewBaseWidth = width;
    overviewBaseHeight = height;
    overviewBasePixelRatio = pixelRatio;
  }

  function mainGraphIsVisible() {
    const panel = timetableGraph.closest("[data-view]");
    return !panel?.hidden && !panel?.classList.contains("workspace-collapsed");
  }

  function drawOverviewViewport(context, width, height) {
    if (!mainGraphIsVisible()) {
      return;
    }
    const mainWidth = timetableGraph.clientWidth;
    const mainHeight = timetableGraph.clientHeight;
    if (!mainWidth || !mainHeight) {
      return;
    }
    const topLeft = overviewTransform.apply(transform.invert([0, 0]));
    const bottomRight = overviewTransform.apply(
      transform.invert([mainWidth, mainHeight]),
    );
    const colours = graphColours();
    context.save();
    context.beginPath();
    context.rect(0, 0, width, height);
    context.clip();
    context.fillStyle = colours.muted;
    context.globalAlpha = 0.16;
    context.fillRect(
      topLeft[0],
      topLeft[1],
      bottomRight[0] - topLeft[0],
      bottomRight[1] - topLeft[1],
    );
    context.globalAlpha = 0.7;
    context.strokeStyle = colours.muted;
    context.lineWidth = 2;
    context.strokeRect(
      topLeft[0],
      topLeft[1],
      bottomRight[0] - topLeft[0],
      bottomRight[1] - topLeft[1],
    );
    context.restore();
  }

  function drawOverviewRangeSelection(context) {
    if (!overviewRangeSelection) {
      return;
    }
    const [startX, startY] = overviewRangeSelection.start;
    const [endX, endY] = overviewRangeSelection.end;
    const left = Math.min(startX, endX);
    const top = Math.min(startY, endY);
    const width = Math.abs(endX - startX);
    const height = Math.abs(endY - startY);
    const colours = graphColours();
    context.save();
    context.fillStyle = colours.selected;
    context.globalAlpha = 0.12;
    context.fillRect(left, top, width, height);
    context.globalAlpha = 0.9;
    context.strokeStyle = colours.selected;
    context.lineWidth = 2;
    context.setLineDash([5, 4]);
    context.strokeRect(left, top, width, height);
    context.restore();
  }

  function drawGraphOverview() {
    const panel = timetableGraphOverview.closest("[data-view]");
    if (
      !currentModel ||
      panel?.hidden ||
      panel?.classList.contains("workspace-collapsed")
    ) {
      return;
    }
    const { context, width, height, pixelRatio } = overviewCanvasAndContext();
    if (!currentModel.nodes.length) {
      context.clearRect(0, 0, width, height);
      return;
    }
    if (
      overviewBaseModel !== currentModel ||
      overviewBaseWidth !== width ||
      overviewBaseHeight !== height ||
      overviewBasePixelRatio !== pixelRatio
    ) {
      rebuildOverviewBase(width, height, pixelRatio);
    }
    context.clearRect(0, 0, width, height);
    context.drawImage(overviewBaseCanvas, 0, 0, width, height);
    drawOverviewViewport(context, width, height);
    drawOverviewRangeSelection(context);
  }

  function graphPoint(event) {
    const [screenX, screenY] = pointerPosition(
      event,
      timetableGraph.querySelector("canvas"),
    );
    return transform.invert([screenX, screenY]);
  }

  function directNodeAtPoint(x, y) {
    return currentModel.spatialIndex
      .query(x, y)
      .reverse()
      .find((node) => !isExpandedContainer(node) && nodeContains(node, x, y));
  }

  function expandedContainerAtPoint(x, y) {
    return currentModel.nodes
      .filter((node) => isExpandedContainer(node) && nodeContains(node, x, y))
      .sort(
        (left, right) =>
          left.halfWidth * left.halfHeight - right.halfWidth * right.halfHeight,
      )[0];
  }

  function nearestNode(event, maximumDistance = 20) {
    if (!currentModel) {
      return null;
    }
    const [x, y] = graphPoint(event);
    const outlineTolerance = 4 / transform.k;
    for (const node of currentModel.nodes.filter(isExpandedContainer)) {
      if (distanceToOutline(node, x, y) <= outlineTolerance) {
        return node;
      }
    }
    const directNode = directNodeAtPoint(x, y);
    if (directNode) {
      return directNode;
    }
    return currentModel.spatialIndex
      .query(x, y, maximumDistance / transform.k)
      .reduce(
        (nearest, node) => {
          const distance = distanceToNode(node, x, y);
          return distance < nearest.distance ? { node, distance } : nearest;
        },
        { node: null, distance: maximumDistance / transform.k },
      ).node;
  }

  function nodeDescription(node) {
    if (node.kind === "tensor") {
      const detail = tensorColourDescription(node.tensor);
      return `${node.tensor.id}\nTensor · ${formatBytes(node.tensor.num_bytes)} · ${node.tensor.dtype} [${node.tensor.shape.join(" × ")}]${detail ? `\n${detail}` : ""}`;
    }
    if (node.kind === "group") {
      return `${node.group.id}\nCompute group · ${node.group.visibleMembers.length} visible of ${node.group.members.length} partitions · ${formatCount(groupMachineOps(node.memberIndices))} machine ops · ${node.compute.op}`;
    }
    if (node.kind === "layer") {
      return `${node.layer}\nLayer · ${node.memberIndices.length.toLocaleString()} compute nodes · ${node.layerGroups.length.toLocaleString()} compute groups`;
    }
    return `${node.compute.id}\nCompute · ${node.compute.op} · ${formatCount(node.compute.machine_ops)} machine ops · ${node.compute.pe} · ${node.compute.layer}`;
  }

  function tensorColourDescription(tensor) {
    if (tensorColourMode === "role") {
      const role = tensorRoleDefinitions.find(
        (definition) => definition.id === tensorRoles.get(tensor.id),
      );
      return `Data-flow role · ${role?.label || "Unconnected"}`;
    }
    if (tensorColourMode === "traffic") {
      const traffic = cachedTensorTraffic(tensor);
      return `Filtered Read · ${formatBytes(traffic.readBytes)} (${traffic.readRatio.toFixed(2)}×) · Written · ${formatBytes(traffic.writtenBytes)} (${traffic.writeRatio.toFixed(2)}×)`;
    }
    if (tensorColourMode === "memory") {
      const memories = tensorMemories.get(tensor.id) || [];
      return `Memory · ${memories.length ? memories.join(", ") : "Unallocated"}`;
    }
    return tensorColourMode === "dtype"
      ? `Data type · ${tensor.dtype}`
      : "Uniform tensor colour";
  }

  function appendTensorLegendItem(label, colour, className = "") {
    const item = document.createElement("span");
    const swatch = document.createElement("i");
    swatch.className = className;
    if (colour) {
      swatch.style.background = colour;
    }
    item.append(swatch, document.createTextNode(label));
    timetableGraphControls.tensorLegend.append(item);
  }

  function renderTensorColourLegend() {
    const legend = timetableGraphControls.tensorLegend;
    legend.replaceChildren();
    if (tensorColourMode === "role") {
      const present = new Set(tensorRoles.values());
      for (const definition of tensorRoleDefinitions) {
        if (present.has(definition.id)) {
          appendTensorLegendItem(definition.label, definition.colour);
        }
      }
      return;
    }
    if (tensorColourMode === "traffic") {
      appendTensorLegendItem("Filtered Read / Written", null, "traffic");
      appendTensorLegendItem("More than 1×", null, "repeated");
      return;
    }
    if (tensorColourMode === "dtype") {
      for (const [dtype, colour] of dtypeColours) {
        appendTensorLegendItem(dtype, colour);
      }
      return;
    }
    if (tensorColourMode === "memory") {
      for (const [memory, colour] of memoryColours) {
        appendTensorLegendItem(memory, colour);
      }
      if ([...tensorMemories.values()].some((memories) => !memories.length)) {
        appendTensorLegendItem("Unallocated", "var(--panel)");
      }
      return;
    }
    appendTensorLegendItem("Tensor", "var(--muted)");
  }

  function setTimetableGraphTensorColourMode(mode) {
    const option = [...timetableGraphControls.tensorColour.options].find(
      (candidate) => candidate.value === mode && !candidate.disabled,
    );
    tensorColourMode = option?.value || "role";
    timetableGraphControls.tensorColour.value = tensorColourMode;
    overviewBaseModel = null;
    renderTensorColourLegend();
    requestDraw();
  }

  function setTimetableGraphGroupSizeMode(mode) {
    groupSizeMode = mode === "machine-ops" ? mode : "nodes";
    timetableGraphControls.groupSize.value = groupSizeMode;
    timetableGraphControls.groupSizeLegend.lastChild.textContent =
      groupSizeMode === "machine-ops"
        ? "Compute group area: machine ops"
        : "Compute group area: nodes";
    if (currentModel) {
      rebuildGraph();
    } else {
      modelDirty = true;
    }
  }

  function setTimetableGraphComputeLaneMode(mode) {
    if (typeof mode === "boolean") {
      mode = mode ? "operation" : "none";
    }
    const option = [...timetableGraphControls.computeLanes.options].find(
      (candidate) => candidate.value === mode,
    );
    computeLaneMode = option?.value || "operation";
    timetableGraphControls.computeLanes.value = computeLaneMode;
    if (currentModel) {
      rebuildGraph();
    } else {
      modelDirty = true;
    }
  }

  function setTimetableGraphEdgeDisplayMode(mode) {
    edgeDisplayMode = mode === "all" ? "all" : "selection";
    timetableGraphControls.edgesSelection.setAttribute(
      "aria-pressed",
      String(edgeDisplayMode === "selection"),
    );
    timetableGraphControls.edgesAll.setAttribute(
      "aria-pressed",
      String(edgeDisplayMode === "all"),
    );
    requestDraw();
  }

  function setTimetableGraphGroupRows(value) {
    const rows = Number(value);
    computeGroupMaxRows = Number.isFinite(rows)
      ? Math.max(1, Math.min(MAX_VISIBLE_NODES, Math.trunc(rows)))
      : DEFAULT_EXPANDED_GROUP_MAX_ROWS;
    timetableGraphControls.groupRows.value = String(computeGroupMaxRows);
    if (currentModel) {
      rebuildGraph();
    } else {
      modelDirty = true;
    }
  }

  function setTimetableGraphNodeScale(kind, value) {
    if (!Object.hasOwn(nodeScales, kind)) return;
    const scale = Number(value);
    const next =
      Number.isFinite(scale) && scale > 0
        ? Math.max(0.1, Math.min(10, scale))
        : 1;
    timetableGraphControls[`${kind}Scale`].value = String(next);
    if (nodeScales[kind] === next) return;
    nodeScales[kind] = next;
    if (currentModel) {
      rebuildGraph();
    } else {
      modelDirty = true;
    }
  }

  function showTooltip(event, node) {
    const tooltip = timetableGraph.querySelector(".timetable-graph-tooltip");
    if (!node) {
      tooltip.hidden = true;
      return;
    }
    tooltip.textContent = nodeDescription(node);
    tooltip.hidden = false;
    const bounds = timetableGraph.getBoundingClientRect();
    tooltip.style.left = `${Math.min(event.clientX - bounds.left + 12, bounds.width - tooltip.offsetWidth - 8)}px`;
    tooltip.style.top = `${Math.min(event.clientY - bounds.top + 12, bounds.height - tooltip.offsetHeight - 8)}px`;
  }

  function selectVisualNode(node) {
    if (node.kind === "tensor") {
      App.selectGraphTensor(node.tensor);
    } else if (node.kind === "layer") {
      App.selectGraphLayer(node.layer);
    } else if (node.kind === "group") {
      App.selectComputeGroup({ ...node.group, index: node.group.index });
    } else {
      App.selectCompute(node.compute);
    }
  }

  function rebuildHierarchy(rollback, label) {
    const model = buildVisibleModel();
    if (model.overLimit) {
      rollback();
      timetableGraphStatus.textContent = `Cannot expand ${label}: ${model.overLimit.toLocaleString()} visible nodes would exceed the ${MAX_VISIBLE_NODES.toLocaleString()}-node limit. Collapse another layer or group first.`;
      return false;
    }
    cancelPendingFocus();
    rebuildGraph(false, model);
    App.selectionChanged("node");
    return true;
  }

  function expandGroup(group) {
    if (group.members.length < 2) {
      return;
    }
    const layer = computeNodes[group.members[0]].layer;
    if (expandedGroups.has(group.index) && expandedLayers.has(layer)) {
      return;
    }
    const groupWasExpanded = expandedGroups.has(group.index);
    const layerWasExpanded = expandedLayers.has(layer);
    expandedGroups.add(group.index);
    expandedLayers.add(layer);
    rebuildHierarchy(() => {
      if (!groupWasExpanded) {
        expandedGroups.delete(group.index);
      }
      if (!layerWasExpanded) {
        expandedLayers.delete(layer);
      }
    }, group.id);
  }

  function collapseGroup(group) {
    if (expandedGroups.delete(group.index)) {
      cancelPendingFocus();
      rebuildGraph();
      App.selectionChanged("node");
    }
  }

  function expandLayer(layer) {
    if (expandedLayers.has(layer)) {
      return;
    }
    expandedLayers.add(layer);
    rebuildHierarchy(() => expandedLayers.delete(layer), layer);
  }

  function collapseLayer(layer) {
    if (expandedLayers.delete(layer)) {
      cancelPendingFocus();
      rebuildGraph();
      App.selectionChanged("node");
    }
  }

  function expandedLayerNodeCount(layerNode) {
    return (
      layerNode.layerTensorCount +
      layerNode.layerGroups.reduce(
        (count, { group, visibleMembers }) =>
          count +
          (group.members.length > 1 && expandedGroups.has(group.index)
            ? visibleMembers.length + 1
            : 1),
        0,
      )
    );
  }

  function tryExpandLayer(layerNode, visibleNodeCount) {
    if (expandedLayers.has(layerNode.layer)) {
      return { visibleNodeCount, changed: false, skipped: false };
    }
    const nextCount = visibleNodeCount + expandedLayerNodeCount(layerNode);
    if (nextCount > MAX_VISIBLE_NODES) {
      return { visibleNodeCount, changed: false, skipped: true };
    }
    expandedLayers.add(layerNode.layer);
    return { visibleNodeCount: nextCount, changed: true, skipped: false };
  }

  function tryExpandGroup(group, visibleMembers, visibleNodeCount) {
    if (group.members.length < 2 || expandedGroups.has(group.index)) {
      return { visibleNodeCount, changed: false, skipped: false };
    }
    const nextCount = visibleNodeCount + visibleMembers.length;
    if (nextCount > MAX_VISIBLE_NODES) {
      return { visibleNodeCount, changed: false, skipped: true };
    }
    expandedGroups.add(group.index);
    return { visibleNodeCount: nextCount, changed: true, skipped: false };
  }

  function finishBulkExpansion(changed, skipped, emptyMessage) {
    if (!changed) {
      timetableGraphStatus.textContent = skipped
        ? `No further nodes can be expanded without exceeding the ${MAX_VISIBLE_NODES.toLocaleString()}-node limit.`
        : emptyMessage;
      return;
    }
    cancelPendingFocus();
    const model = buildVisibleModel();
    rebuildGraph(false, model);
    App.selectionChanged("node");
    const message = skipped
      ? `Expanded ${changed.toLocaleString()} containers; some remain folded because the ${MAX_VISIBLE_NODES.toLocaleString()}-node limit was reached.`
      : `Expanded ${changed.toLocaleString()} containers.`;
    window.requestAnimationFrame(() => {
      timetableGraphStatus.textContent = `${graphStatus()} · ${message}`;
    });
  }

  function nodeIntersectsViewport(node, width, height) {
    const bounds = timetableGraphNodeBounds(node);
    const left = bounds.left * transform.k + transform.x;
    const right = bounds.right * transform.k + transform.x;
    const top = bounds.top * transform.k + transform.y;
    const bottom = bounds.bottom * transform.k + transform.y;
    return right >= 0 && left <= width && bottom >= 0 && top <= height;
  }

  function expandVisible() {
    if (!currentModel?.nodes.length || currentModel.overLimit) {
      return;
    }
    const { width, height } = canvasAndContext();
    const candidates = currentModel.nodes
      .filter(
        (node) =>
          ((node.kind === "layer" && !expandedLayers.has(node.layer)) ||
            (node.kind === "group" && !expandedGroups.has(node.group.index))) &&
          nodeIntersectsViewport(node, width, height),
      )
      .sort((left, right) => left.x - right.x || left.y - right.y);
    let visibleNodeCount = currentModel.nodes.length;
    let changed = 0;
    let skipped = 0;
    for (const node of candidates) {
      const result =
        node.kind === "layer"
          ? tryExpandLayer(node, visibleNodeCount)
          : tryExpandGroup(node.group, node.memberIndices, visibleNodeCount);
      visibleNodeCount = result.visibleNodeCount;
      changed += Number(result.changed);
      skipped += Number(result.skipped);
    }
    finishBulkExpansion(
      changed,
      skipped,
      "No collapsed layers or compute groups are visible.",
    );
  }

  function expandAll() {
    if (!currentModel?.nodes.length || currentModel.overLimit) {
      return;
    }
    const layerNodes = currentModel.nodes
      .filter((node) => node.kind === "layer")
      .sort(
        (left, right) =>
          left.x - right.x ||
          left.layoutRank - right.layoutRank ||
          left.label.localeCompare(right.label, undefined, { numeric: true }),
      );
    const visibleGroupNodes = new Map(
      currentModel.nodes
        .filter((node) => node.kind === "group")
        .map((node) => [node.group.index, node]),
    );
    let visibleNodeCount = currentModel.nodes.length;
    let changed = 0;
    let skipped = 0;
    const collapsedLayers = layerNodes.filter(
      (layerNode) => !expandedLayers.has(layerNode.layer),
    );
    if (collapsedLayers.length) {
      for (const layerNode of collapsedLayers) {
        const result = tryExpandLayer(layerNode, visibleNodeCount);
        visibleNodeCount = result.visibleNodeCount;
        changed += Number(result.changed);
        skipped += Number(result.skipped);
      }
      finishBulkExpansion(changed, skipped, "All visible layers are expanded.");
      return;
    }

    for (const layerNode of layerNodes) {
      const groups = [...layerNode.layerGroups].sort((left, right) => {
        const leftNode = visibleGroupNodes.get(left.group.index);
        const rightNode = visibleGroupNodes.get(right.group.index);
        return (
          (leftNode?.x ?? layerNode.x) - (rightNode?.x ?? layerNode.x) ||
          left.group.id.localeCompare(right.group.id, undefined, {
            numeric: true,
          })
        );
      });
      for (const { group, visibleMembers } of groups) {
        const groupResult = tryExpandGroup(
          group,
          visibleMembers,
          visibleNodeCount,
        );
        visibleNodeCount = groupResult.visibleNodeCount;
        changed += Number(groupResult.changed);
        skipped += Number(groupResult.skipped);
      }
    }
    finishBulkExpansion(
      changed,
      skipped,
      "All visible compute groups are expanded.",
    );
  }

  function collapseAll() {
    let changed = expandedGroups.size;
    let kind = "compute groups";
    if (changed) {
      expandedGroups.clear();
    } else {
      changed = expandedLayers.size;
      kind = "layers";
      expandedLayers.clear();
    }
    if (!changed) {
      timetableGraphStatus.textContent = "All layers are already collapsed.";
      return;
    }
    cancelPendingFocus();
    rebuildGraph();
    App.selectionChanged("node");
    window.requestAnimationFrame(() => {
      timetableGraphStatus.textContent = `${graphStatus()} · Collapsed ${changed.toLocaleString()} ${kind}.`;
    });
  }

  function fitGraph() {
    if (!currentModel?.nodes.length) {
      return;
    }
    const { width, height } = canvasAndContext();
    const bounds = currentModel.bounds;
    const graphWidth = Math.max(bounds.right - bounds.left + 60, 1);
    const graphHeight = Math.max(bounds.bottom - bounds.top + 60, 1);
    const scale = Math.min(width / graphWidth, height / graphHeight, 2);
    graphViewport.setTransform(
      createViewportTransform(
        width / 2 - ((bounds.left + bounds.right) / 2) * scale,
        height / 2 - ((bounds.top + bounds.bottom) / 2) * scale,
        scale,
      ),
    );
  }

  function visualNodeForSelection(selection, model = currentModel) {
    if (!selection || !model) {
      return null;
    }
    if (selection.kind === "group") {
      const group = computeGroups[Number(selection.id)];
      if (!group) {
        return null;
      }
      return (
        model.visualById.get(`g:${group.index}`) ||
        group.members
          .map((member) =>
            model.nodeByActualKey.get(
              actualKey("compute", computeNodes[member]?.id),
            ),
          )
          .find(Boolean)
      );
    }
    if (selection.kind === "layer") {
      return model.visualById.get(`l:${selection.id}`);
    }
    return model.nodeByActualKey.get(actualKey(selection.kind, selection.id));
  }

  function setHoveredGraphNode(node) {
    if (node?.id === hoveredNode?.id) {
      return;
    }
    hoveredNode = node;
    requestDraw();
  }

  function centreOnSelection(selection, animate = true) {
    if (timetableGraph.closest("[data-view]")?.hidden) {
      return false;
    }
    const node = visualNodeForSelection(selection);
    if (!node) {
      return false;
    }
    const { width, height } = canvasAndContext();
    const scale = transform.k;
    const centred = createViewportTransform(
      width / 2 - node.x * scale,
      height / 2 - node.y * scale,
      scale,
    );
    graphViewport.setTransform(centred, { animate, duration: 250 });
    return true;
  }

  function centreGraphAt(x, y) {
    const { width, height } = canvasAndContext();
    const centred = createViewportTransform(
      width / 2 - x * transform.k,
      height / 2 - y * transform.k,
      transform.k,
    );
    graphViewport.setTransform(centred);
  }

  function overviewScreenPoint(event) {
    const canvas = timetableGraphOverview.querySelector("canvas");
    const [x, y] = pointerPosition(event, canvas);
    return [
      Math.max(0, Math.min(canvas.clientWidth, x)),
      Math.max(0, Math.min(canvas.clientHeight, y)),
    ];
  }

  function centreGraphFromOverview(event) {
    const point = overviewTransform.invert(overviewScreenPoint(event));
    centreGraphAt(point[0], point[1]);
  }

  function fitGraphToOverviewRange() {
    if (!overviewRangeSelection) {
      return;
    }
    const [startX, startY] = overviewRangeSelection.start;
    const [endX, endY] = overviewRangeSelection.end;
    if (Math.abs(endX - startX) < 4 || Math.abs(endY - startY) < 4) {
      return;
    }
    const topLeft = overviewTransform.invert([
      Math.min(startX, endX),
      Math.min(startY, endY),
    ]);
    const bottomRight = overviewTransform.invert([
      Math.max(startX, endX),
      Math.max(startY, endY),
    ]);
    const { width, height } = canvasAndContext();
    const padding = 12;
    const rangeWidth = Math.max(bottomRight[0] - topLeft[0], 1);
    const rangeHeight = Math.max(bottomRight[1] - topLeft[1], 1);
    const [minimumScale, maximumScale] = graphViewport.scaleExtent();
    const scale = Math.max(
      minimumScale,
      Math.min(
        maximumScale,
        Math.min(
          Math.max(width - padding * 2, 1) / rangeWidth,
          Math.max(height - padding * 2, 1) / rangeHeight,
        ),
      ),
    );
    const centreX = (topLeft[0] + bottomRight[0]) / 2;
    const centreY = (topLeft[1] + bottomRight[1]) / 2;
    const selected = createViewportTransform(
      width / 2 - centreX * scale,
      height / 2 - centreY * scale,
      scale,
    );
    graphViewport.setTransform(selected);
  }

  function cancelPendingFocus() {
    pendingFocusSelection = null;
    graphViewport.cancelAnimation();
  }

  function focusTimetableGraphSelection() {
    pendingFocusSelection = state.selectedNode
      ? { ...state.selectedNode }
      : null;
    if (!pendingFocusSelection) {
      return;
    }
    if (centreOnSelection(pendingFocusSelection)) {
      pendingFocusSelection = null;
    }
  }

  function zoomGraphBy(factor, animate = true) {
    graphViewport.scaleBy(factor, { animate, duration: 160 });
  }

  function graphStatus() {
    if (currentModel.overLimit) {
      return `${currentModel.overLimit.toLocaleString()} nodes match the current filters, exceeding the ${MAX_VISIBLE_NODES.toLocaleString()}-node limit. Narrow the filters to render the graph.`;
    }
    if (!currentModel.nodes.length) {
      return "No tensor or compute nodes match the current filters.";
    }
    const folded = currentModel.nodes.filter(
      (node) => node.kind === "group" && !node.expanded,
    ).length;
    const collapsedLayers = currentModel.nodes.filter(
      (node) => node.kind === "layer" && !node.expanded,
    ).length;
    const selection = state.selectedNode;
    const selectedLabel = selection ? ` · Selected: ${selection.id}` : "";
    const rankWarning = currentModel.forwardRanksComplete
      ? ""
      : " · Some folded groups contain conflicting dependencies";
    return `${currentModel.nodes.length.toLocaleString()} nodes · ${currentModel.links.length.toLocaleString()} links · ${collapsedLayers.toLocaleString()} collapsed layers · ${folded.toLocaleString()} folded compute groups${rankWarning}${selectedLabel}`;
  }

  function rebuildGraph(fitAfterLayout = false, preparedModel = null) {
    currentModel = preparedModel || buildVisibleModel();
    modelDirty = false;
    renderedModel = currentModel;
    timetableGraphStatus.textContent = graphStatus();
    if (fitAfterLayout) {
      fitGraph();
    } else {
      requestDraw();
    }
  }

  function selectionExists(selection, model) {
    if (!selection) {
      return false;
    }
    if (selection.kind === "group") {
      const group = computeGroups[Number(selection.id)];
      return Boolean(
        group?.members.some((member) =>
          model.context.computeIndices.has(member),
        ),
      );
    }
    if (selection.kind === "layer") {
      return model.visualById.has(`l:${selection.id}`);
    }
    return model.nodeByActualKey.has(actualKey(selection.kind, selection.id));
  }

  function syncSelectedNode() {
    const model = visibleModel();
    if (selectionExists(state.selectedNode, model)) {
      return;
    }
    const tensor = model.nodes.find((node) => node.kind === "tensor");
    const node = tensor || model.nodes[0];
    if (!node) {
      state.selectedNode = null;
    } else if (node.kind === "tensor") {
      state.selectedNode = { kind: "tensor", id: node.tensor.id };
    } else if (node.kind === "group") {
      state.selectedNode = {
        kind: "group",
        id: groupSelectionId(node.group),
      };
    } else if (node.kind === "layer") {
      state.selectedNode = { kind: "layer", id: node.layer };
    } else {
      state.selectedNode = { kind: "compute", id: node.compute.id };
    }
  }

  function renderTimetableGraph() {
    const model = visibleModel();
    if (renderedModel !== model) {
      rebuildGraph(!renderedModel && !pendingFocusSelection, model);
    } else {
      timetableGraphStatus.textContent = graphStatus();
      requestDraw();
    }
    if (pendingFocusSelection) {
      centreOnSelection(pendingFocusSelection, false);
      pendingFocusSelection = null;
    }
  }

  function visibleModel() {
    if (modelDirty || !currentModel) {
      currentModel = buildVisibleModel();
      modelDirty = false;
    }
    return currentModel;
  }

  function invalidateTimetableGraph() {
    modelDirty = true;
  }

  function viewGeometry(tensor, access) {
    const view = access.view === undefined ? null : tensor.views?.[access.view];
    return view
      ? `offsets [${view.offsets.join(", ")}], shape [${view.shape.join(" × ")}], ${formatBytes(view.num_bytes)}`
      : `full tensor, ${formatBytes(tensor.num_bytes)}`;
  }

  function descriptorForSelection(selection) {
    if (!selection) {
      return null;
    }
    if (selection.kind === "tensor") {
      const tensor = tensorsById.get(selection.id);
      return tensor ? { kind: "tensor", id: tensor.id, tensor } : null;
    }
    if (selection.kind === "compute") {
      const compute = computeById.get(selection.id);
      return compute ? { kind: "compute", id: compute.id, compute } : null;
    }
    if (selection.kind === "layer") {
      return data.layers?.some((layer) => layer.name === selection.id)
        ? { kind: "layer", id: selection.id, layer: selection.id }
        : null;
    }
    const group = computeGroups[Number(selection.id)];
    return group ? { kind: "group", id: groupSelectionId(group), group } : null;
  }

  function visualDescriptorForActual(key, model) {
    const visual = model.nodeByActualKey.get(key);
    if (!visual) {
      return null;
    }
    if (visual.kind === "tensor") {
      return { kind: "tensor", id: visual.tensor.id, tensor: visual.tensor };
    }
    if (visual.kind === "group") {
      return {
        kind: "group",
        id: groupSelectionId(visual.group),
        group: visual.group,
      };
    }
    if (visual.kind === "layer") {
      return {
        kind: "layer",
        id: visual.layer,
        layer: visual.layer,
      };
    }
    return { kind: "compute", id: visual.compute.id, compute: visual.compute };
  }

  function descriptorLabel(descriptor) {
    return descriptor.kind === "tensor"
      ? descriptor.tensor.id
      : descriptor.kind === "compute"
        ? descriptor.compute.id
        : descriptor.kind === "group"
          ? descriptor.group.id
          : descriptor.layer;
  }

  function descriptorType(descriptor) {
    if (descriptor.kind === "group") {
      return "Compute group";
    }
    if (descriptor.kind === "layer") {
      return "Layer";
    }
    return descriptor.kind === "tensor" ? "Tensor" : "Compute";
  }

  function selectionActualKeys(descriptor, model) {
    if (descriptor.kind === "tensor") {
      return [actualKey("tensor", descriptor.tensor.id)];
    }
    if (descriptor.kind === "compute") {
      return [actualKey("compute", descriptor.compute.id)];
    }
    if (descriptor.kind === "layer") {
      const layer = model.visualById.get(`l:${descriptor.layer}`);
      return (layer?.memberIndices || []).map((index) =>
        actualKey("compute", computeNodes[index].id),
      );
    }
    return descriptor.group.members
      .filter((member) => model.context.computeIndices.has(member))
      .map((member) => actualKey("compute", computeNodes[member].id));
  }

  function selectedConnections(descriptor, model) {
    const selectedKeys = new Set(selectionActualKeys(descriptor, model));
    const inputs = new Map();
    const outputs = new Map();
    for (const selectedKey of selectedKeys) {
      for (const edge of adjacency.get(selectedKey) || []) {
        const incoming = edge.targetKey === selectedKey;
        const otherKey = incoming ? edge.sourceKey : edge.targetKey;
        if (selectedKeys.has(otherKey)) {
          continue;
        }
        const other = visualDescriptorForActual(otherKey, model);
        if (!other) {
          continue;
        }
        const collection = incoming ? inputs : outputs;
        const key = `${other.kind}:${other.id}:${edge.type}`;
        const existing = collection.get(key);
        if (existing) {
          existing.count += 1;
          existing.edges.push(edge);
        } else {
          collection.set(key, {
            other,
            type: edge.type,
            count: 1,
            edges: [edge],
          });
        }
      }
    }
    return { inputs: [...inputs.values()], outputs: [...outputs.values()] };
  }

  function syncNavigationState(descriptor, connections) {
    const selectionKey = `${descriptor.kind}:${descriptor.id}`;
    if (navigationSelectionKey !== selectionKey) {
      navigationSelectionKey = selectionKey;
      navigationIndex = 0;
    }
    const activeConnections = connections[navigationDirection];
    navigationIndex = activeConnections.length
      ? Math.min(navigationIndex, activeConnections.length - 1)
      : 0;
  }

  function highlightedConnection(connections) {
    return connections[navigationDirection][navigationIndex] || null;
  }

  function selectDescriptor(descriptor) {
    if (descriptor.kind === "tensor") {
      App.selectGraphTensor(descriptor.tensor);
    } else if (descriptor.kind === "compute") {
      App.selectCompute(descriptor.compute);
    } else if (descriptor.kind === "layer") {
      App.selectGraphLayer(descriptor.layer);
    } else {
      App.selectComputeGroup({
        ...descriptor.group,
        index: descriptor.group.index,
      });
    }
  }

  function connectionDetail(connection) {
    const edge = connection.edges[0];
    const type =
      connection.type === "read"
        ? "Read"
        : connection.type === "write"
          ? "Written"
          : "Control";
    const details = [type];
    if (edge.access) {
      const slot = edge.access.slot;
      details.push(slot === undefined ? "unspecified port" : `port ${slot}`);
      details.push(viewGeometry(edge.tensor, edge.access));
    }
    if (connection.count > 1) {
      details.push(`${connection.count.toLocaleString()} edges`);
    }
    return details.join(" · ");
  }

  function connectionsMarkup(title, side, connections) {
    const activeSide = navigationDirection === side;
    if (!connections.length) {
      return `<section class="${activeSide ? "navigation-active" : ""}"><h3>${title}</h3><p>No visible ${title.toLowerCase()}.</p></section>`;
    }
    return `
      <section class="${activeSide ? "navigation-active" : ""}">
        <h3>${title}</h3>
        <div class="selected-node-connections">
          ${connections
            .map(
              (connection, index) => `
                <button type="button" data-node-neighbour="${side}:${index}" class="${activeSide && navigationIndex === index ? "navigation-highlighted" : ""}"${activeSide && navigationIndex === index ? ' aria-current="true"' : ""}>
                  <strong>${escapeHtml(descriptorLabel(connection.other))}</strong>
                  <span>${escapeHtml(descriptorType(connection.other))} · ${escapeHtml(connectionDetail(connection))}</span>
                </button>`,
            )
            .join("")}
        </div>
      </section>`;
  }

  function selectedNodeSummary(descriptor, model) {
    if (descriptor.kind === "tensor") {
      const tensor = descriptor.tensor;
      return `
        <strong>${escapeHtml(tensor.id)}</strong>
        <span>Tensor · ${escapeHtml(tensor.dtype)} [${escapeHtml(tensor.shape.join(" × "))}] · ${formatBytes(tensor.num_bytes)}</span>`;
    }
    if (descriptor.kind === "compute") {
      const compute = descriptor.compute;
      const group = groupByCompute[computeIndexById.get(compute.id)];
      const collapse =
        group?.members.length > 1 &&
        expandedGroups.has(group.index) &&
        expandedLayers.has(compute.layer)
          ? `<button type="button" data-collapse-selected-group>Collapse group</button>`
          : "";
      return `
        <strong>${escapeHtml(compute.id)}</strong>
        <span>Compute · ${escapeHtml(compute.op)} · ${escapeHtml(compute.pe)} · ${escapeHtml(compute.layer)}</span>
        ${collapse}`;
    }
    if (descriptor.kind === "layer") {
      const layerNode = model.visualById.get(`l:${descriptor.layer}`);
      const computeIndices = layerNode?.memberIndices || [];
      const pes = new Set(
        computeIndices.map((index) => computeNodes[index].pe),
      );
      const operations = new Set(
        computeIndices.map((index) => computeNodes[index].op),
      );
      const action = expandedLayers.has(descriptor.layer)
        ? `<button type="button" data-collapse-selected-layer>Collapse layer</button>`
        : `<button type="button" data-expand-selected-layer>Expand layer</button>`;
      return `
        <strong>${escapeHtml(descriptor.layer)}</strong>
        <span>Layer · ${computeIndices.length.toLocaleString()} compute nodes · ${(layerNode?.layerGroups.length || 0).toLocaleString()} groups · ${operations.size.toLocaleString()} operations · ${pes.size.toLocaleString()} PEs</span>
        ${action}`;
    }
    const group = descriptor.group;
    const visible = group.members.filter((member) =>
      model.context.computeIndices.has(member),
    );
    const pes = new Set(visible.map((member) => computeNodes[member].pe));
    const operation =
      computeNodes[visible[0] ?? group.members[0]]?.op || "compute";
    const layer = computeNodes[group.members[0]].layer;
    const action =
      expandedGroups.has(group.index) && expandedLayers.has(layer)
        ? `<button type="button" data-collapse-selected-group>Collapse group</button>`
        : `<button type="button" data-expand-selected-group>Expand group</button>`;
    return `
      <strong>${escapeHtml(group.id)}</strong>
      <span>Compute group · ${escapeHtml(operation)} · ${visible.length.toLocaleString()} visible of ${group.members.length.toLocaleString()} partitions · ${formatCount(groupMachineOps(visible))} machine ops · ${pes.size.toLocaleString()} PEs</span>
      ${action}`;
  }

  function renderSelectedNode() {
    const model = visibleModel();
    const descriptor = descriptorForSelection(state.selectedNode);
    if (!descriptor || !selectionExists(state.selectedNode, model)) {
      navigationTargetSelection = null;
      navigationCandidateSelections = [];
      selectedNodePanel.innerHTML = `<p>No tensor or compute node matches the current filters.</p>`;
      return;
    }
    const connections = selectedConnections(descriptor, model);
    syncNavigationState(descriptor, connections);
    const highlighted = highlightedConnection(connections);
    navigationTargetSelection = highlighted
      ? { kind: highlighted.other.kind, id: highlighted.other.id }
      : null;
    navigationCandidateSelections = [
      ...connections.inputs,
      ...connections.outputs,
    ].map((connection) => ({
      kind: connection.other.kind,
      id: connection.other.id,
    }));
    requestDraw();
    selectedNodePanel.innerHTML = `
      <div class="selected-node-summary">${selectedNodeSummary(descriptor, model)}</div>
      <div class="selected-node-columns">
        ${connectionsMarkup("Inputs", "inputs", connections.inputs)}
        ${connectionsMarkup("Outputs", "outputs", connections.outputs)}
      </div>`;

    for (const button of selectedNodePanel.querySelectorAll(
      "[data-node-neighbour]",
    )) {
      const [side, index] = button.dataset.nodeNeighbour.split(":");
      const connection = connections[side]?.[Number(index)];
      if (!connection) {
        continue;
      }
      button.addEventListener("click", () => {
        navigationDirection = side;
        navigationIndex = Number(index);
        selectDescriptor(connection.other);
      });
      const kind =
        connection.other.kind === "group" ? "compute" : connection.other.kind;
      markEntityElement(button, kind, descriptorLabel(connection.other));
      const graphNode = visualNodeForSelection(
        { kind: connection.other.kind, id: connection.other.id },
        model,
      );
      button.addEventListener("pointerenter", () => {
        setHoveredGraphNode(graphNode);
      });
      button.addEventListener("pointerleave", () => {
        if (hoveredNode === graphNode) {
          setHoveredGraphNode(null);
        }
      });
    }
    selectedNodePanel
      .querySelector("[data-expand-selected-group]")
      ?.addEventListener("click", () => expandGroup(descriptor.group));
    selectedNodePanel
      .querySelector("[data-collapse-selected-group]")
      ?.addEventListener("click", () => {
        const group =
          descriptor.kind === "group"
            ? descriptor.group
            : groupByCompute[computeIndexById.get(descriptor.compute.id)];
        collapseGroup(group);
      });
    selectedNodePanel
      .querySelector("[data-expand-selected-layer]")
      ?.addEventListener("click", () => expandLayer(descriptor.layer));
    selectedNodePanel
      .querySelector("[data-collapse-selected-layer]")
      ?.addEventListener("click", () => collapseLayer(descriptor.layer));
  }

  function selectedNavigationContext() {
    const model = visibleModel();
    const descriptor = descriptorForSelection(state.selectedNode);
    if (!descriptor || !selectionExists(state.selectedNode, model)) {
      return null;
    }
    const connections = selectedConnections(descriptor, model);
    syncNavigationState(descriptor, connections);
    return { descriptor, connections };
  }

  function cycleNavigation(delta) {
    const context = selectedNavigationContext();
    const connections = context?.connections[navigationDirection] || [];
    if (!connections.length) {
      return;
    }
    navigationIndex =
      (navigationIndex + delta + connections.length) % connections.length;
    renderSelectedNode();
  }

  function navigateInDirection(direction) {
    const context = selectedNavigationContext();
    if (!context) {
      return;
    }
    if (navigationDirection !== direction) {
      navigationDirection = direction;
      navigationIndex = 0;
      renderSelectedNode();
      return;
    }
    const connection = highlightedConnection(context.connections);
    if (connection) {
      selectDescriptor(connection.other);
    }
  }

  function toggleSelectedNode() {
    const context = selectedNavigationContext();
    if (!context) {
      return;
    }
    const descriptor = context.descriptor;
    if (descriptor.kind === "layer") {
      if (expandedLayers.has(descriptor.layer)) {
        collapseLayer(descriptor.layer);
      } else {
        expandLayer(descriptor.layer);
      }
      return;
    }
    if (descriptor.kind !== "group" && descriptor.kind !== "compute") {
      return;
    }
    const group =
      descriptor.kind === "group"
        ? descriptor.group
        : groupByCompute[computeIndexById.get(descriptor.compute.id)];
    const layer = computeNodes[group?.members[0]]?.layer;
    if (!group || group.members.length < 2) {
      if (layer && !expandedLayers.has(layer)) {
        expandLayer(layer);
      }
      return;
    }
    if (!expandedLayers.has(layer)) {
      expandLayer(layer);
    } else if (expandedGroups.has(group.index)) {
      collapseGroup(group);
    } else {
      expandGroup(group);
    }
  }

  function handleGraphKeydown(event) {
    const actions = {
      "+": () => zoomGraphBy(Math.SQRT2),
      "=": () => zoomGraphBy(Math.SQRT2),
      "-": () => zoomGraphBy(1 / Math.SQRT2),
      ArrowUp: () => cycleNavigation(-1),
      ArrowDown: () => cycleNavigation(1),
      ArrowLeft: () => navigateInDirection("inputs"),
      ArrowRight: () => navigateInDirection("outputs"),
      Enter: toggleSelectedNode,
    };
    const action = actions[event.key];
    if (!action || event.altKey || event.ctrlKey || event.metaKey) {
      return;
    }
    event.preventDefault();
    action();
  }

  function bindGraphInteractions() {
    if (controlsBound) {
      return;
    }
    controlsBound = true;
    const canvas = timetableGraph.querySelector("canvas");
    graphViewport = createCanvasViewport(canvas, {
      minScale: 0.005,
      maxScale: 8,
      onChange: (next) => {
        transform = next;
        requestDraw();
      },
    });
    canvas.addEventListener("keydown", handleGraphKeydown);
    canvas.addEventListener("pointermove", (event) => {
      const node = nearestNode(event);
      if (node?.id !== hoveredNode?.id) {
        setHoveredGraphNode(node);
        const kind =
          node?.kind === "tensor"
            ? "tensor"
            : node?.kind === "layer"
              ? "layer"
              : node
                ? "compute"
                : null;
        setHoveredEntity(kind, node?.label || null);
      }
      showTooltip(event, node);
    });
    canvas.addEventListener("pointerleave", () => {
      setHoveredGraphNode(null);
      setHoveredEntity(null, null);
      showTooltip(null, null);
    });
    canvas.addEventListener("click", (event) => {
      canvas.focus({ preventScroll: true });
      const node = nearestNode(event);
      if (node) {
        selectVisualNode(node);
      }
    });
    canvas.addEventListener("dblclick", (event) => {
      const [x, y] = graphPoint(event);
      const node =
        directNodeAtPoint(x, y) ||
        expandedContainerAtPoint(x, y) ||
        nearestNode(event);
      if (node?.kind === "layer") {
        event.preventDefault();
        if (node.expanded) {
          collapseLayer(node.layer);
        } else {
          expandLayer(node.layer);
        }
      } else if (node?.kind === "group") {
        event.preventDefault();
        if (node.expanded) {
          collapseGroup(node.group);
        } else {
          expandGroup(node.group);
        }
      } else if (
        node?.kind === "compute" &&
        node.group.members.length > 1 &&
        expandedGroups.has(node.group.index)
      ) {
        event.preventDefault();
        collapseGroup(node.group);
      }
    });
    const overviewCanvas = timetableGraphOverview.querySelector("canvas");
    overviewCanvas.addEventListener(
      "wheel",
      (event) => {
        event.preventDefault();
        if (overviewPointerId !== null) return;
        const delta =
          event.deltaY *
          (event.deltaMode === 1
            ? 16
            : event.deltaMode === 2
              ? timetableGraphOverview.clientHeight
              : 1);
        zoomGraphBy(2 ** Math.max(-1, Math.min(1, -delta * 0.002)), false);
      },
      { passive: false },
    );
    overviewCanvas.addEventListener("keydown", (event) => {
      if (event.altKey || event.ctrlKey || event.metaKey) return;
      if (!["+", "=", "-"].includes(event.key)) return;
      event.preventDefault();
      zoomGraphBy(event.key === "-" ? 1 / Math.SQRT2 : Math.SQRT2);
    });
    overviewCanvas.addEventListener("pointerdown", (event) => {
      if (event.button !== 0) {
        return;
      }
      event.preventDefault();
      overviewCanvas.focus({ preventScroll: true });
      overviewPointerId = event.pointerId;
      overviewCanvas.setPointerCapture(event.pointerId);
      if (event.ctrlKey) {
        const point = overviewScreenPoint(event);
        overviewRangeSelection = { start: point, end: point };
        requestDraw();
      } else {
        centreGraphFromOverview(event);
      }
    });
    overviewCanvas.addEventListener("pointermove", (event) => {
      if (event.pointerId !== overviewPointerId) {
        return;
      }
      if (overviewRangeSelection) {
        overviewRangeSelection.end = overviewScreenPoint(event);
        requestDraw();
      } else {
        centreGraphFromOverview(event);
      }
    });
    const finishOverviewPointer = (event) => {
      if (event.pointerId !== overviewPointerId) {
        return;
      }
      if (event.type === "pointerup" && overviewRangeSelection) {
        overviewRangeSelection.end = overviewScreenPoint(event);
        fitGraphToOverviewRange();
      }
      overviewRangeSelection = null;
      overviewPointerId = null;
      if (overviewCanvas.hasPointerCapture(event.pointerId)) {
        overviewCanvas.releasePointerCapture(event.pointerId);
      }
      requestDraw();
    };
    overviewCanvas.addEventListener("pointerup", finishOverviewPointer);
    overviewCanvas.addEventListener("pointercancel", finishOverviewPointer);
    overviewCanvas.addEventListener("contextmenu", (event) => {
      event.preventDefault();
    });
    timetableGraphControls.fit.addEventListener("click", fitGraph);
    timetableGraphControls.zoomOut.addEventListener("click", () => {
      zoomGraphBy(1 / Math.SQRT2);
    });
    timetableGraphControls.zoomIn.addEventListener("click", () => {
      zoomGraphBy(Math.SQRT2);
    });
    timetableGraphControls.reset.addEventListener("click", () => {
      rebuildGraph(true);
    });
    timetableGraphControls.expandVisible.addEventListener(
      "click",
      expandVisible,
    );
    timetableGraphControls.expandAll.addEventListener("click", expandAll);
    timetableGraphControls.collapse.addEventListener("click", collapseAll);
    timetableGraphControls.computeLanes.addEventListener("change", () => {
      setTimetableGraphComputeLaneMode(
        timetableGraphControls.computeLanes.value,
      );
      App.workspaceChanged?.();
    });
    timetableGraphControls.edgesSelection.addEventListener("click", () => {
      setTimetableGraphEdgeDisplayMode("selection");
      App.workspaceChanged?.();
    });
    timetableGraphControls.edgesAll.addEventListener("click", () => {
      setTimetableGraphEdgeDisplayMode("all");
      App.workspaceChanged?.();
    });
    const applyGroupRows = () => {
      setTimetableGraphGroupRows(timetableGraphControls.groupRows.value);
      App.workspaceChanged?.();
    };
    for (const kind of Object.keys(nodeScales)) {
      const input = timetableGraphControls[`${kind}Scale`];
      const applyScale = () => {
        setTimetableGraphNodeScale(kind, input.value);
        App.workspaceChanged?.();
      };
      input.addEventListener("change", applyScale);
      input.addEventListener("keydown", (event) => {
        if (event.key === "Enter") {
          event.preventDefault();
          applyScale();
        }
      });
    }
    timetableGraphControls.groupRows.addEventListener("change", applyGroupRows);
    timetableGraphControls.groupRows.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        applyGroupRows();
      }
    });
    timetableGraphControls.groupSize.addEventListener("change", () => {
      setTimetableGraphGroupSizeMode(timetableGraphControls.groupSize.value);
      App.workspaceChanged?.();
    });
    const memoryOption = [...timetableGraphControls.tensorColour.options].find(
      (option) => option.value === "memory",
    );
    if (memoryOption) {
      memoryOption.disabled = !platformMemories.length;
    }
    timetableGraphControls.tensorColour.addEventListener("change", () => {
      setTimetableGraphTensorColourMode(
        timetableGraphControls.tensorColour.value,
      );
      App.workspaceChanged?.();
    });
    setTimetableGraphTensorColourMode(tensorColourMode);
    setTimetableGraphGroupSizeMode(groupSizeMode);
    setTimetableGraphComputeLaneMode(computeLaneMode);
    setTimetableGraphEdgeDisplayMode(edgeDisplayMode);
    setTimetableGraphGroupRows(computeGroupMaxRows);
    resizeObserver = new ResizeObserver(() => requestDraw());
    resizeObserver.observe(timetableGraph);
    resizeObserver.observe(timetableGraphOverview);
    window.addEventListener("gwr-hover-change", requestDraw);
  }

  bindGraphInteractions();

  Object.assign(App, {
    graphSelectionModel: visibleModel,
    graphSelectionExists: selectionExists,
    renderTimetableGraph,
    renderSelectedNode,
    syncSelectedNode,
    invalidateTimetableGraph,
    focusTimetableGraphSelection,
    setTimetableGraphTensorColourMode,
    setTimetableGraphGroupSizeMode,
    setTimetableGraphComputeLaneMode,
    setTimetableGraphComputeLanes: setTimetableGraphComputeLaneMode,
    setTimetableGraphEdgeDisplayMode,
    setTimetableGraphGroupRows,
    setTimetableGraphNodeScale,
  });
})();
