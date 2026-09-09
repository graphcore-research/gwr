// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

(() => {
  const App = window.GWR_VISUALISATION_APP;
  const COLUMN_STEP = 120;
  const ITEM_GAP = 18;
  const LAYER_HEADER = 28;
  const CONTAINER_PADDING = 14;
  const LANE_GAP = 24;
  const SWEEP_COUNT = 6;
  const SPATIAL_CELL_SIZE = 128;

  function naturalCompare(left, right) {
    return left.localeCompare(right, undefined, { numeric: true });
  }

  function areaScaledRadius(ratio, minimum, maximum) {
    const boundedRatio = Math.max(0, Math.min(Number(ratio) || 0, 1));
    return Math.sqrt(
      minimum ** 2 + boundedRatio * (maximum ** 2 - minimum ** 2),
    );
  }

  function tensorPlacementForEdges(edges) {
    const readLayers = new Set(
      edges
        .filter((edge) => edge.type === "read")
        .map((edge) => edge.compute?.layer)
        .filter(Boolean),
    );
    if (!readLayers.size) {
      return null;
    }
    const writeLayers = new Set(
      edges
        .filter((edge) => edge.type === "write")
        .map((edge) => edge.compute?.layer)
        .filter(Boolean),
    );
    if (writeLayers.size === 1) {
      const producerLayer = [...writeLayers][0];
      if (readLayers.has(producerLayer)) {
        return {
          layer: producerLayer,
          role: readLayers.size === 1 ? "intra" : "transfer",
        };
      }
    }
    if (!writeLayers.size && readLayers.size === 1) {
      return { layer: [...readLayers][0], role: "input" };
    }
    return null;
  }

  function tensorNeedsBypass(edges, layerIndex) {
    const readers = edges
      .filter((edge) => edge.type === "read")
      .map((edge) => layerIndex(edge.compute?.layer))
      .filter(Number.isFinite);
    if (!readers.length) {
      return false;
    }
    const writers = edges
      .filter((edge) => edge.type === "write")
      .map((edge) => layerIndex(edge.compute?.layer))
      .filter(Number.isFinite);
    const sourceLayer = writers.length
      ? Math.max(...writers)
      : Math.min(...readers) - 1;
    return readers.some((readerLayer) => readerLayer > sourceLayer + 1);
  }

  function tensorRoleForEdges(edges, placement, bypassed) {
    if (placement?.role === "input") {
      return "layer-input";
    }
    if (placement?.role === "intra") {
      return "intra-layer";
    }
    if (placement?.role === "transfer") {
      return "layer-transfer";
    }
    if (bypassed) {
      return "long-span";
    }
    const hasRead = edges.some((edge) => edge.type === "read");
    const hasWrite = edges.some((edge) => edge.type === "write");
    if (hasRead && !hasWrite) {
      return "graph-input";
    }
    if (hasWrite && !hasRead) {
      return "graph-output";
    }
    return hasRead && hasWrite ? "layer-transfer" : "unconnected";
  }

  function nodeHalfWidth(node) {
    return node.halfWidth ?? node.radius ?? 0;
  }

  function nodeHalfHeight(node) {
    return node.halfHeight ?? node.radius ?? 0;
  }

  function nodeBounds(node, padding = 0) {
    if (!padding && node.bounds) {
      return node.bounds;
    }
    return {
      left: node.x - nodeHalfWidth(node) - padding,
      right: node.x + nodeHalfWidth(node) + padding,
      top: node.y - nodeHalfHeight(node) - padding,
      bottom: node.y + nodeHalfHeight(node) + padding,
    };
  }

  function columnsFor(items) {
    const columns = new Map();
    for (const item of items) {
      if (!columns.has(item.layoutRank)) {
        columns.set(item.layoutRank, []);
      }
      columns.get(item.layoutRank).push(item);
    }
    return [...columns]
      .sort(([left], [right]) => left - right)
      .map(([rank, nodes]) => ({ rank, nodes }));
  }

  function anchorFor(node) {
    return node.layoutAnchor || node;
  }

  function neighboursFor(links) {
    const neighbours = new Map();
    function add(from, to, weight) {
      if (!neighbours.has(from)) {
        neighbours.set(from, []);
      }
      neighbours.get(from).push({ node: to, weight });
    }
    for (const link of links) {
      const source = anchorFor(link.source);
      const target = anchorFor(link.target);
      if (source === target) {
        continue;
      }
      add(source, target, link.count || 1);
      add(target, source, link.count || 1);
    }
    return neighbours;
  }

  function stackColumn(column, startY) {
    let cursor = startY;
    for (const node of column.nodes) {
      cursor += nodeHalfHeight(node);
      node.y = cursor;
      cursor += nodeHalfHeight(node) + ITEM_GAP;
    }
    return Math.max(cursor - startY - ITEM_GAP, 0);
  }

  function columnHeight(column) {
    return column.nodes.reduce(
      (height, node, index) =>
        height + nodeHalfHeight(node) * 2 + (index ? ITEM_GAP : 0),
      0,
    );
  }

  function neighbourBarycentre(node, neighbours, direction) {
    let weightedY = 0;
    let totalWeight = 0;
    for (const neighbour of neighbours.get(node) || []) {
      const rankDelta = neighbour.node.layoutRank - node.layoutRank;
      if (
        (direction > 0 && rankDelta >= 0) ||
        (direction < 0 && rankDelta <= 0)
      ) {
        continue;
      }
      weightedY += neighbour.node.y * neighbour.weight;
      totalWeight += neighbour.weight;
    }
    return totalWeight ? weightedY / totalWeight : null;
  }

  function orderColumn(column, neighbours, direction) {
    column.nodes.sort((left, right) => {
      const leftBarycentre = neighbourBarycentre(left, neighbours, direction);
      const rightBarycentre = neighbourBarycentre(right, neighbours, direction);
      if (leftBarycentre !== null && rightBarycentre !== null) {
        return (
          leftBarycentre - rightBarycentre ||
          left.layoutOrder - right.layoutOrder
        );
      }
      if (leftBarycentre !== null) {
        return -1;
      }
      if (rightBarycentre !== null) {
        return 1;
      }
      return left.layoutOrder - right.layoutOrder;
    });
  }

  function orderColumns(columns, links) {
    for (const column of columns) {
      column.nodes.sort(
        (left, right) =>
          left.layoutOrder - right.layoutOrder ||
          naturalCompare(left.label, right.label),
      );
      stackColumn(column, 0);
    }
    const neighbours = neighboursFor(links);
    for (let pass = 0; pass < SWEEP_COUNT; pass += 1) {
      const direction = pass % 2 ? -1 : 1;
      const ordered = direction > 0 ? columns : [...columns].reverse();
      for (const column of ordered) {
        orderColumn(column, neighbours, direction);
        stackColumn(column, 0);
      }
    }
  }

  function positionColumns(columns) {
    const maximumHeight = Math.max(0, ...columns.map(columnHeight));
    let previousRight = -Infinity;
    let previousLayer = null;
    for (const [index, column] of columns.entries()) {
      const height = columnHeight(column);
      const halfWidth = Math.max(0, ...column.nodes.map(nodeHalfWidth));
      const layers = new Set(
        column.nodes
          .map((node) => node.topNode)
          .filter((node) => node?.kind === "layer"),
      );
      const layer = layers.size === 1 ? [...layers][0] : null;
      const boundaryPadding =
        layer !== previousLayer
          ? Number(Boolean(previousLayer)) * CONTAINER_PADDING +
            Number(Boolean(layer)) * CONTAINER_PADDING
          : 0;
      const x = Math.max(
        index * COLUMN_STEP,
        previousRight + ITEM_GAP + boundaryPadding + halfWidth,
      );
      stackColumn(column, (maximumHeight - height) / 2);
      for (const node of column.nodes) {
        node.x = x;
      }
      previousRight = x + halfWidth;
      previousLayer = layer;
    }
  }

  function gridGeometry(children, maximumRows, gap, padding) {
    const columns = [];
    for (let index = 0; index < children.length; index += maximumRows) {
      columns.push(children.slice(index, index + maximumRows));
    }
    const columnWidths = columns.map((column) =>
      Math.max(0, ...column.map((child) => nodeHalfWidth(child) * 2)),
    );
    const rowCount = Math.min(children.length, maximumRows);
    const rowHeights = Array.from({ length: rowCount }, (_, row) =>
      Math.max(
        0,
        ...columns.map((column) =>
          column[row] ? nodeHalfHeight(column[row]) * 2 : 0,
        ),
      ),
    );
    return {
      columns,
      columnWidths,
      rowHeights,
      gap,
      padding,
      width:
        padding * 2 +
        columnWidths.reduce((total, width) => total + width, 0) +
        Math.max(columns.length - 1, 0) * gap,
      height:
        padding * 2 +
        rowHeights.reduce((total, height) => total + height, 0) +
        Math.max(rowCount - 1, 0) * gap,
    };
  }

  function stackedHeight(nodes) {
    return nodes.reduce(
      (height, node, index) =>
        height + nodeHalfHeight(node) * 2 + (index ? ITEM_GAP : 0),
      0,
    );
  }

  function intervalsOverlap(top, bottom, interval) {
    return top < interval.bottom + ITEM_GAP && bottom > interval.top - ITEM_GAP;
  }

  function nearestAvailableY(preferredY, halfHeight, occupied) {
    const candidates = [preferredY];
    for (const interval of occupied) {
      candidates.push(
        interval.top - ITEM_GAP - halfHeight,
        interval.bottom + ITEM_GAP + halfHeight,
      );
    }
    return candidates
      .filter((candidate) =>
        occupied.every(
          (interval) =>
            !intervalsOverlap(
              candidate - halfHeight,
              candidate + halfHeight,
              interval,
            ),
        ),
      )
      .sort(
        (left, right) =>
          Math.abs(left - preferredY) - Math.abs(right - preferredY) ||
          left - right,
      )[0];
  }

  function positionComputeLanes(model, columns) {
    if (!model.computeLanes) {
      model.lanes = [];
      return;
    }
    const laneOrder = new Map(
      model.computeLanes.map((lane, index) => [lane, index]),
    );
    const laneHeights = new Map(model.computeLanes.map((lane) => [lane, 0]));
    for (const column of columns) {
      const byLane = new Map();
      for (const node of column.nodes.filter((item) => item.layoutLane)) {
        const laneNodes = byLane.get(node.layoutLane) || [];
        laneNodes.push(node);
        byLane.set(node.layoutLane, laneNodes);
      }
      for (const [lane, nodes] of byLane) {
        laneHeights.set(
          lane,
          Math.max(laneHeights.get(lane) || 0, stackedHeight(nodes)),
        );
      }
    }

    let cursor = 0;
    model.lanes = model.computeLanes.map((lane) => {
      const height = Math.max(laneHeights.get(lane) || 0, 1);
      const result = {
        id: lane,
        top: cursor,
        bottom: cursor + height,
        centre: cursor + height / 2,
      };
      cursor += height + LANE_GAP;
      return result;
    });
    for (const column of columns) {
      const occupied = [];
      const laneNodes = column.nodes
        .filter((node) => node.layoutLane)
        .sort(
          (left, right) =>
            laneOrder.get(left.layoutLane) - laneOrder.get(right.layoutLane) ||
            left.layoutOrder - right.layoutOrder,
        );
      for (const lane of model.lanes) {
        const nodes = laneNodes.filter((node) => node.layoutLane === lane.id);
        let laneCursor = lane.centre - stackedHeight(nodes) / 2;
        for (const node of nodes) {
          node.y = laneCursor + nodeHalfHeight(node);
          laneCursor += nodeHalfHeight(node) * 2 + ITEM_GAP;
          occupied.push(nodeBounds(node));
        }
      }
      for (const node of column.nodes.filter((item) => !item.layoutLane)) {
        node.y = nearestAvailableY(node.y, nodeHalfHeight(node), occupied);
        occupied.push(nodeBounds(node));
      }
    }
  }

  function layerChild(node, layer) {
    if (node.topNode !== layer) {
      return null;
    }
    return node.parentGroup || node;
  }

  function rankLayerChildren(layer, links) {
    const children = layer.layoutChildren;
    const childSet = new Set(children);
    const outgoing = new Map(children.map((child) => [child, new Set()]));
    const incomingCount = new Map(children.map((child) => [child, 0]));

    for (const link of links) {
      const source = layerChild(link.source, layer);
      const target = layerChild(link.target, layer);
      if (
        !source ||
        !target ||
        source === target ||
        !childSet.has(source) ||
        !childSet.has(target) ||
        outgoing.get(source).has(target)
      ) {
        continue;
      }
      outgoing.get(source).add(target);
      incomingCount.set(target, incomingCount.get(target) + 1);
    }

    const depth = new Map(children.map((child) => [child, 0]));
    const ready = children
      .filter((child) => incomingCount.get(child) === 0)
      .sort((left, right) => left.layoutOrder - right.layoutOrder);
    for (let index = 0; index < ready.length; index += 1) {
      const source = ready[index];
      for (const target of outgoing.get(source)) {
        depth.set(target, Math.max(depth.get(target), depth.get(source) + 1));
        const remaining = incomingCount.get(target) - 1;
        incomingCount.set(target, remaining);
        if (remaining === 0) {
          ready.push(target);
        }
      }
    }

    for (const child of children) {
      child.layoutRank = layer.layoutRank + depth.get(child);
    }
  }

  function rankExpandedLayers(model) {
    for (const layer of model.nodes.filter(
      (node) => node.kind === "layer" && node.expanded,
    )) {
      rankLayerChildren(layer, model.links);
    }
  }

  function enforceForwardRanks(model) {
    const nodes = [...new Set(model.layoutItems.map(anchorFor))];
    const nodeSet = new Set(nodes);
    const outgoing = new Map(nodes.map((node) => [node, new Set()]));
    const incomingCount = new Map(nodes.map((node) => [node, 0]));
    for (const link of model.links) {
      const source = anchorFor(link.source);
      const target = anchorFor(link.target);
      if (
        source === target ||
        !nodeSet.has(source) ||
        !nodeSet.has(target) ||
        outgoing.get(source).has(target)
      ) {
        continue;
      }
      outgoing.get(source).add(target);
      incomingCount.set(target, incomingCount.get(target) + 1);
    }

    const ready = nodes
      .filter((node) => incomingCount.get(node) === 0)
      .sort(
        (left, right) =>
          left.layoutRank - right.layoutRank ||
          left.layoutOrder - right.layoutOrder,
      );
    for (let index = 0; index < ready.length; index += 1) {
      const source = ready[index];
      for (const target of outgoing.get(source)) {
        target.layoutRank = Math.max(target.layoutRank, source.layoutRank + 1);
        const remaining = incomingCount.get(target) - 1;
        incomingCount.set(target, remaining);
        if (remaining === 0) {
          ready.push(target);
        }
      }
    }
    return ready.length === nodes.length;
  }

  function positionGroupChildren(model) {
    for (const group of model.nodes.filter(
      (node) => node.kind === "group" && node.expanded,
    )) {
      if (group.grid) {
        let x = group.x - group.halfWidth + group.grid.padding;
        for (const [columnIndex, column] of group.grid.columns.entries()) {
          const columnWidth = group.grid.columnWidths[columnIndex];
          let y = group.y - group.halfHeight + group.grid.padding;
          for (const [row, child] of column.entries()) {
            child.x = x + columnWidth / 2;
            child.y = y + group.grid.rowHeights[row] / 2;
            y += group.grid.rowHeights[row] + group.grid.gap;
          }
          x += columnWidth + group.grid.gap;
        }
        continue;
      }
      let cursor =
        group.y -
        group.halfHeight +
        (group.contentPadding ?? CONTAINER_PADDING);
      for (const child of group.children) {
        cursor += child.radius;
        child.x = group.x;
        child.y = cursor;
        cursor += child.radius + ITEM_GAP / 2;
      }
    }
  }

  function positionLayerInputs(model) {
    for (const layer of model.nodes.filter(
      (node) => node.kind === "layer" && node.expanded,
    )) {
      const content = layer.layoutChildren.filter(
        (node) => !node.layerInputSide,
      );
      if (!content.length) {
        continue;
      }
      const contentTop = Math.min(
        ...content.map((node) => node.y - nodeHalfHeight(node)),
      );
      const contentBottom = Math.max(
        ...content.map((node) => node.y + nodeHalfHeight(node)),
      );
      let topCursor = contentTop - ITEM_GAP;
      let bottomCursor = contentBottom + ITEM_GAP;
      for (const input of layer.layoutChildren.filter(
        (node) => node.layerInputSide === "top",
      )) {
        input.y = topCursor - nodeHalfHeight(input);
        topCursor = input.y - nodeHalfHeight(input) - ITEM_GAP / 2;
      }
      for (const input of layer.layoutChildren.filter(
        (node) => node.layerInputSide === "bottom",
      )) {
        input.y = bottomCursor + nodeHalfHeight(input);
        bottomCursor = input.y + nodeHalfHeight(input) + ITEM_GAP / 2;
      }
    }
  }

  function positionLayerContainers(model) {
    for (const layer of model.nodes.filter(
      (node) => node.kind === "layer" && node.expanded,
    )) {
      const children = layer.layoutChildren;
      const left = Math.min(
        ...children.map((node) => node.x - nodeHalfWidth(node)),
      );
      const right = Math.max(
        ...children.map((node) => node.x + nodeHalfWidth(node)),
      );
      const top = Math.min(
        ...children.map((node) => node.y - nodeHalfHeight(node)),
      );
      const bottom = Math.max(
        ...children.map((node) => node.y + nodeHalfHeight(node)),
      );
      layer.x = (left + right) / 2;
      layer.y = (top - LAYER_HEADER + bottom) / 2;
      layer.halfWidth = Math.max((right - left) / 2 + CONTAINER_PADDING, 50);
      layer.halfHeight = Math.max(
        (bottom - top + LAYER_HEADER) / 2 + CONTAINER_PADDING,
        42,
      );
    }
  }

  function positionTopLevelBlocks(model) {
    const rootByMember = new Map();
    for (const layer of model.nodes.filter(
      (node) => node.kind === "layer" && node.expanded,
    )) {
      rootByMember.set(layer, layer);
      for (const child of layer.layoutChildren) {
        rootByMember.set(child, layer);
        for (const groupChild of child.children || []) {
          rootByMember.set(groupChild, layer);
        }
      }
    }
    for (const group of model.nodes.filter(
      (node) => node.kind === "group" && node.expanded,
    )) {
      const root = rootByMember.get(group) || group.topNode || group;
      rootByMember.set(group, root);
      for (const child of group.children) {
        rootByMember.set(child, root);
      }
    }

    const rootFor = (node) => rootByMember.get(node) || node.topNode || node;
    const membersByRoot = new Map();
    for (const node of model.nodes) {
      const root = rootFor(node);
      if (!membersByRoot.has(root)) {
        membersByRoot.set(root, []);
      }
      membersByRoot.get(root).push(node);
    }
    const layers = [...membersByRoot]
      .filter(([root]) => root.kind === "layer")
      .map(([root, members]) => ({ root, members }))
      .sort(
        (left, right) =>
          left.root.layoutOrder - right.root.layoutOrder ||
          naturalCompare(left.root.label, right.root.label),
      );
    const externalTensors = model.nodes.filter(
      (node) => node.kind === "tensor" && rootFor(node) === node,
    );
    const connections = new Map(
      externalTensors.map((tensor) => [
        tensor,
        { producers: new Set(), consumers: new Set() },
      ]),
    );
    for (const link of model.links) {
      const sourceRoot = rootFor(link.source);
      const targetRoot = rootFor(link.target);
      if (connections.has(link.target) && sourceRoot.kind === "layer") {
        connections.get(link.target).producers.add(sourceRoot);
      }
      if (connections.has(link.source) && targetRoot.kind === "layer") {
        connections.get(link.source).consumers.add(targetRoot);
      }
    }

    const layerIndex = new Map(
      layers.map((layer, index) => [layer.root, index]),
    );
    const boundaryGaps = new Map();
    for (const [tensor, { producers, consumers }] of connections) {
      if (tensor.bypassSide || !producers.size || !consumers.size) {
        continue;
      }
      const producerIndex = Math.max(
        ...[...producers].map((layer) => layerIndex.get(layer) ?? -Infinity),
      );
      const consumerIndex = Math.min(
        ...[...consumers].map((layer) => layerIndex.get(layer) ?? Infinity),
      );
      if (consumerIndex === producerIndex + 1) {
        boundaryGaps.set(
          producerIndex,
          Math.max(
            boundaryGaps.get(producerIndex) || ITEM_GAP,
            nodeHalfWidth(tensor) * 2 + ITEM_GAP * 2,
          ),
        );
      }
    }

    const layerBounds = new Map();
    let previousRight = -Infinity;
    for (const [index, block] of layers.entries()) {
      const left = Math.min(
        ...block.members.map((node) => nodeBounds(node).left),
      );
      const right = Math.max(
        ...block.members.map((node) => nodeBounds(node).right),
      );
      const gap = index ? boundaryGaps.get(index - 1) || ITEM_GAP : 0;
      const offset = Math.max(previousRight + gap - left, 0);
      if (offset) {
        for (const node of block.members) {
          node.x += offset;
        }
      }
      previousRight = right + offset;
      layerBounds.set(block.root, {
        left: left + offset,
        right: previousRight,
      });
    }

    for (const [tensor, { producers, consumers }] of connections) {
      const producerRight = Math.max(
        -Infinity,
        ...[...producers]
          .map((layer) => layerBounds.get(layer)?.right)
          .filter(Number.isFinite),
      );
      const consumerLeft = Math.min(
        Infinity,
        ...[...consumers]
          .map((layer) => layerBounds.get(layer)?.left)
          .filter(Number.isFinite),
      );
      if (Number.isFinite(producerRight) && Number.isFinite(consumerLeft)) {
        tensor.x = (producerRight + consumerLeft) / 2;
      } else if (Number.isFinite(producerRight)) {
        tensor.x = producerRight + ITEM_GAP + nodeHalfWidth(tensor);
      } else if (Number.isFinite(consumerLeft)) {
        tensor.x = consumerLeft - ITEM_GAP - nodeHalfWidth(tensor);
      }
    }
  }

  function positionBypassTensors(model) {
    const layers = model.nodes.filter((node) => node.kind === "layer");
    if (!layers.length) {
      return;
    }
    const layerTop = Math.min(...layers.map((node) => nodeBounds(node).top));
    const layerBottom = Math.max(
      ...layers.map((node) => nodeBounds(node).bottom),
    );
    const top = model.nodes.filter(
      (node) => node.kind === "tensor" && node.bypassSide === "top",
    );
    const bottom = model.nodes.filter(
      (node) => node.kind === "tensor" && node.bypassSide === "bottom",
    );
    positionBypassSide(top, layerTop - ITEM_GAP, -1);
    positionBypassSide(bottom, layerBottom + ITEM_GAP, 1);
  }

  function positionBypassSide(tensors, boundary, direction) {
    const lanes = [];
    const ordered = [...tensors].sort(
      (left, right) => left.x - right.x || left.layoutOrder - right.layoutOrder,
    );
    for (const tensor of ordered) {
      const left = tensor.x - nodeHalfWidth(tensor);
      let lane = lanes.find((candidate) => left > candidate.right + ITEM_GAP);
      if (!lane) {
        lane = { nodes: [], right: -Infinity, halfHeight: 0 };
        lanes.push(lane);
      }
      lane.nodes.push(tensor);
      lane.right = tensor.x + nodeHalfWidth(tensor);
      lane.halfHeight = Math.max(lane.halfHeight, nodeHalfHeight(tensor));
    }
    let cursor = boundary;
    for (const lane of lanes) {
      const centre = cursor + direction * lane.halfHeight;
      for (const tensor of lane.nodes) tensor.y = centre;
      cursor = centre + direction * (lane.halfHeight + ITEM_GAP);
    }
  }

  function portOffset(index, count, node) {
    if (count < 2) {
      return 0;
    }
    const maximum = Math.max(nodeHalfHeight(node) - 4, 0);
    const span = Math.min(maximum * 1.5, (count - 1) * 5);
    return -span / 2 + (span * index) / (count - 1);
  }

  function assignPortOffsets(links) {
    const outgoing = new Map();
    const incoming = new Map();
    for (const link of links) {
      if (!outgoing.has(link.source)) outgoing.set(link.source, []);
      if (!incoming.has(link.target)) incoming.set(link.target, []);
      outgoing.get(link.source).push(link);
      incoming.get(link.target).push(link);
    }
    for (const [node, nodeLinks] of outgoing) {
      nodeLinks.sort((left, right) => left.target.y - right.target.y);
      nodeLinks.forEach((link, index) => {
        link.sourceOffset = portOffset(index, nodeLinks.length, node);
      });
    }
    for (const [node, nodeLinks] of incoming) {
      nodeLinks.sort((left, right) => left.source.y - right.source.y);
      nodeLinks.forEach((link, index) => {
        link.targetOffset = portOffset(index, nodeLinks.length, node);
      });
    }
  }

  function horizontalBoundary(node, direction, offset) {
    const yOffset = Math.max(
      -nodeHalfHeight(node) + 2,
      Math.min(nodeHalfHeight(node) - 2, offset || 0),
    );
    if (node.shape !== "circle") {
      return {
        x: node.x + direction * nodeHalfWidth(node),
        y: node.y + yOffset,
      };
    }
    const xOffset = Math.sqrt(Math.max(node.radius ** 2 - yOffset ** 2, 0));
    return { x: node.x + direction * xOffset, y: node.y + yOffset };
  }

  function routeLinks(model) {
    assignPortOffsets(model.links);
    const channels = new Map();
    for (const link of model.links) {
      const sameColumn = link.target.x === link.source.x;
      const direction = link.target.x >= link.source.x ? 1 : -1;
      const start = horizontalBoundary(
        link.source,
        direction,
        link.sourceOffset,
      );
      const end = horizontalBoundary(
        link.target,
        sameColumn ? direction : -direction,
        link.targetOffset,
      );
      const channelKey = `${Math.min(link.source.layoutRank, link.target.layoutRank)}:${Math.max(link.source.layoutRank, link.target.layoutRank)}`;
      const channel = channels.get(channelKey) || 0;
      channels.set(channelKey, channel + 1);
      const bypass = link.source.bypassSide
        ? link.source
        : link.target.bypassSide
          ? link.target
          : null;
      if (bypass && !sameColumn) {
        const approach = Math.min(Math.abs(end.x - start.x) / 3, 60);
        link.route = {
          start,
          control1: {
            x: start.x + direction * approach,
            y: bypass.y,
          },
          control2: {
            x: end.x - direction * approach,
            y: bypass.y,
          },
          end,
        };
      } else if (sameColumn) {
        const detour = Math.max(start.x, end.x) + 44 + (channel % 7) * 7;
        link.route = {
          start,
          control1: { x: detour, y: start.y },
          control2: { x: detour, y: end.y },
          end,
        };
      } else if (direction > 0) {
        const middle = (start.x + end.x) / 2 + ((channel % 7) - 3) * 3;
        link.route = {
          start,
          control1: { x: middle, y: start.y },
          control2: { x: middle, y: end.y },
          end,
        };
      } else {
        const detour = Math.min(start.y, end.y) - 44 - (channel % 7) * 7;
        link.route = {
          start,
          control1: { x: start.x + direction * 50, y: detour },
          control2: { x: end.x - direction * 50, y: detour },
          end,
        };
      }
      link.bounds = routeBounds(link.route);
    }
  }

  function routeBounds(route) {
    const xs = [route.start.x, route.control1.x, route.control2.x, route.end.x];
    const ys = [route.start.y, route.control1.y, route.control2.y, route.end.y];
    return {
      left: Math.min(...xs),
      right: Math.max(...xs),
      top: Math.min(...ys),
      bottom: Math.max(...ys),
    };
  }

  function createSpatialIndex(nodes) {
    const cells = new Map();
    for (const node of nodes.filter((candidate) => !candidate.expanded)) {
      const bounds = nodeBounds(node);
      const minX = Math.floor(bounds.left / SPATIAL_CELL_SIZE);
      const maxX = Math.floor(bounds.right / SPATIAL_CELL_SIZE);
      const minY = Math.floor(bounds.top / SPATIAL_CELL_SIZE);
      const maxY = Math.floor(bounds.bottom / SPATIAL_CELL_SIZE);
      for (let x = minX; x <= maxX; x += 1) {
        for (let y = minY; y <= maxY; y += 1) {
          const key = `${x}:${y}`;
          if (!cells.has(key)) cells.set(key, []);
          cells.get(key).push(node);
        }
      }
    }
    return {
      query(x, y, radius = 0) {
        const found = new Set();
        const minX = Math.floor((x - radius) / SPATIAL_CELL_SIZE);
        const maxX = Math.floor((x + radius) / SPATIAL_CELL_SIZE);
        const minY = Math.floor((y - radius) / SPATIAL_CELL_SIZE);
        const maxY = Math.floor((y + radius) / SPATIAL_CELL_SIZE);
        for (let cellX = minX; cellX <= maxX; cellX += 1) {
          for (let cellY = minY; cellY <= maxY; cellY += 1) {
            for (const node of cells.get(`${cellX}:${cellY}`) || [])
              found.add(node);
          }
        }
        return [...found];
      },
    };
  }

  function layoutTimetableGraph(model) {
    for (const node of model.nodes) {
      delete node.bounds;
    }
    rankExpandedLayers(model);
    model.forwardRanksComplete = enforceForwardRanks(model);
    const columns = columnsFor(model.layoutItems);
    orderColumns(columns, model.links);
    positionColumns(columns);
    positionComputeLanes(model, columns);
    positionGroupChildren(model);
    positionLayerInputs(model);
    positionLayerContainers(model);
    positionTopLevelBlocks(model);
    positionBypassTensors(model);
    for (const node of model.nodes) {
      node.bounds = nodeBounds(node);
    }
    routeLinks(model);
    model.bounds = model.nodes.reduce(
      (bounds, node) => {
        const current = nodeBounds(node);
        return {
          left: Math.min(bounds.left, current.left),
          right: Math.max(bounds.right, current.right),
          top: Math.min(bounds.top, current.top),
          bottom: Math.max(bounds.bottom, current.bottom),
        };
      },
      { left: Infinity, right: -Infinity, top: Infinity, bottom: -Infinity },
    );
    for (const link of model.links) {
      const bounds = link.bounds;
      model.bounds.left = Math.min(model.bounds.left, bounds.left);
      model.bounds.right = Math.max(model.bounds.right, bounds.right);
      model.bounds.top = Math.min(model.bounds.top, bounds.top);
      model.bounds.bottom = Math.max(model.bounds.bottom, bounds.bottom);
    }
    model.spatialIndex = createSpatialIndex(model.nodes);
    return model;
  }

  Object.assign(App, {
    layoutTimetableGraph,
    timetableGraphNodeBounds: nodeBounds,
    timetableGraphRouteBounds: routeBounds,
    timetableGraphTensorNeedsBypass: tensorNeedsBypass,
    timetableGraphTensorPlacement: tensorPlacementForEdges,
    timetableGraphTensorRole: tensorRoleForEdges,
    timetableGraphAreaScaledRadius: areaScaledRadius,
    timetableGraphGridGeometry: gridGeometry,
    timetableGraphEnforceForwardRanks: enforceForwardRanks,
  });
})();
