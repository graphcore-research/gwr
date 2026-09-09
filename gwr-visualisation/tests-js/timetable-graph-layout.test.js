// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

const assert = require("node:assert/strict");
const test = require("node:test");

global.window = { GWR_VISUALISATION_APP: {} };
require("../assets/timetable-graph-layout.js");

const {
  layoutTimetableGraph,
  timetableGraphNodeBounds,
  timetableGraphRouteBounds,
  timetableGraphTensorNeedsBypass,
  timetableGraphTensorPlacement,
  timetableGraphTensorRole,
  timetableGraphAreaScaledRadius,
  timetableGraphGridGeometry,
  timetableGraphEnforceForwardRanks,
} = window.GWR_VISUALISATION_APP;

test("scales node area between fixed group radii", () => {
  assert.equal(timetableGraphAreaScaledRadius(0, 10, 40), 10);
  assert.equal(timetableGraphAreaScaledRadius(1, 10, 40), 40);
  assert.equal(
    timetableGraphAreaScaledRadius(0.5, 10, 40) ** 2,
    10 ** 2 + 0.5 * (40 ** 2 - 10 ** 2),
  );
});

function circle(id, rank, order, radius = 10) {
  const node = {
    id,
    kind: "compute",
    label: id,
    shape: "circle",
    radius,
    layoutRank: rank,
    layoutOrder: order,
  };
  node.layoutAnchor = node;
  return node;
}

function rectangle(id, rank, order, halfWidth, halfHeight, kind = "tensor") {
  const node = {
    id,
    kind,
    label: id,
    shape: "rect",
    halfWidth,
    halfHeight,
    layoutRank: rank,
    layoutOrder: order,
  };
  node.layoutAnchor = node;
  return node;
}

function link(source, target, count = 1) {
  return { source, target, count, type: "read" };
}

function model(nodes, links, layoutItems = nodes) {
  return { nodes, links, layoutItems };
}

test("places ranks left to right and separates nodes vertically", () => {
  const first = circle("first", 2, 0, 12);
  const second = circle("second", 2, 1, 18);
  const tensor = rectangle("tensor", 3, 0, 9, 80);
  const consumer = circle("consumer", 4, 0, 14);

  layoutTimetableGraph(
    model(
      [first, second, tensor, consumer],
      [link(first, tensor), link(tensor, consumer)],
    ),
  );

  assert.ok(first.x < tensor.x && tensor.x < consumer.x);
  const ordered = [first, second].sort((left, right) => left.y - right.y);
  assert.ok(
    timetableGraphNodeBounds(ordered[0]).bottom <
      timetableGraphNodeBounds(ordered[1]).top,
  );
  assert.equal(timetableGraphNodeBounds(tensor).bottom - tensor.y, 80);
});

test("barycentric sweeps align crossing dependencies", () => {
  const leftA = circle("left-a", 0, 0);
  const leftB = circle("left-b", 0, 1);
  const rightA = circle("right-a", 2, 0);
  const rightB = circle("right-b", 2, 1);

  layoutTimetableGraph(
    model(
      [leftA, leftB, rightA, rightB],
      [link(leftA, rightB), link(leftB, rightA)],
    ),
  );

  assert.ok((leftA.y - leftB.y) * (rightB.y - rightA.y) > 0);
});

test("repairs ranks so every DAG edge flows left to right", () => {
  const producer = circle("producer", 4, 0);
  const tensor = rectangle("tensor", 2, 0, 12, 24);
  const consumer = circle("consumer", 1, 0);
  const graph = model(
    [producer, tensor, consumer],
    [link(producer, tensor), link(tensor, consumer)],
  );

  assert.equal(timetableGraphEnforceForwardRanks(graph), true);
  assert.ok(producer.layoutRank < tensor.layoutRank);
  assert.ok(tensor.layoutRank < consumer.layoutRank);

  layoutTimetableGraph(graph);
  assert.ok(producer.x < tensor.x);
  assert.ok(tensor.x < consumer.x);
  for (const edge of graph.links) {
    assert.ok(edge.route.start.x < edge.route.end.x);
  }
});

test("aligns matching operations in horizontal lanes", () => {
  const leftAdd = circle("left-add", 0, 0);
  const rightAdd = circle("right-add", 2, 0);
  const multiply = circle("multiply", 0, 1);
  leftAdd.layoutLane = "add";
  rightAdd.layoutLane = "add";
  multiply.layoutLane = "multiply";
  const graph = model([leftAdd, rightAdd, multiply], [link(leftAdd, rightAdd)]);
  graph.computeLanes = ["add", "multiply"];

  layoutTimetableGraph(graph);

  assert.equal(leftAdd.y, rightAdd.y);
  assert.ok(leftAdd.y < multiply.y);
  assert.deepEqual(
    graph.lanes.map((lane) => lane.id),
    ["add", "multiply"],
  );
});

test("moves tensors clear of operation lanes in the same column", () => {
  const compute = circle("compute", 0, 0, 16);
  compute.layoutLane = "custom";
  const tensor = rectangle("tensor", 0, 1, 12, 24);
  const graph = model([compute, tensor], []);
  graph.computeLanes = ["custom"];

  layoutTimetableGraph(graph);

  const computeBounds = timetableGraphNodeBounds(compute);
  const tensorBounds = timetableGraphNodeBounds(tensor);
  assert.ok(
    tensorBounds.bottom < computeBounds.top ||
      tensorBounds.top > computeBounds.bottom,
  );
});

test("expanded groups and layers contain their children", () => {
  const childA = circle("child-a", 2, 0, 8);
  const childB = circle("child-b", 2, 1, 12);
  const group = rectangle("group", 2, 0, 38, 44, "group");
  group.expanded = true;
  group.children = [childA, childB];
  childA.layoutAnchor = group;
  childB.layoutAnchor = group;
  const layer = rectangle("layer", 2, 0, 50, 42, "layer");
  layer.expanded = true;
  layer.layoutChildren = [group];

  layoutTimetableGraph(model([layer, group, childA, childB], [], [group]));

  const groupBounds = timetableGraphNodeBounds(group);
  const layerBounds = timetableGraphNodeBounds(layer);
  for (const child of [childA, childB]) {
    const bounds = timetableGraphNodeBounds(child);
    assert.ok(
      bounds.top >= groupBounds.top && bounds.bottom <= groupBounds.bottom,
    );
  }
  assert.ok(
    groupBounds.left >= layerBounds.left &&
      groupBounds.right <= layerBounds.right,
  );
  assert.ok(
    groupBounds.top >= layerBounds.top &&
      groupBounds.bottom <= layerBounds.bottom,
  );
});

test("centres expanded group children between equal vertical padding", () => {
  const childA = circle("child-a", 2, 0, 8);
  const childB = circle("child-b", 2, 1, 12);
  const group = rectangle("group", 2, 0, 38, 34.5, "group");
  group.expanded = true;
  group.contentPadding = 10;
  group.children = [childA, childB];
  childA.layoutAnchor = group;
  childB.layoutAnchor = group;

  layoutTimetableGraph(model([group, childA, childB], [], [group]));

  const bounds = timetableGraphNodeBounds(group);
  assert.equal(childA.y - childA.radius - bounds.top, 10);
  assert.equal(bounds.bottom - (childB.y + childB.radius), 10);
});

test("wraps expanded compute groups after 32 rows", () => {
  const children = Array.from({ length: 65 }, (_, index) =>
    circle(`child-${index}`, 2, index, 8 + (index % 3)),
  );
  const grid = timetableGraphGridGeometry(children, 32, 9, 10);
  const group = rectangle(
    "group",
    2,
    0,
    grid.width / 2,
    grid.height / 2,
    "group",
  );
  group.expanded = true;
  group.children = children;
  group.grid = grid;
  for (const child of children) child.layoutAnchor = group;

  layoutTimetableGraph(model([group, ...children], [], [group]));

  assert.deepEqual(
    grid.columns.map((column) => column.length),
    [32, 32, 1],
  );
  assert.equal(children[0].y, children[32].y);
  assert.equal(children[0].x < children[32].x, true);
  const bounds = timetableGraphNodeBounds(group);
  for (const child of children) {
    const childBounds = timetableGraphNodeBounds(child);
    assert.ok(childBounds.left >= bounds.left);
    assert.ok(childBounds.right <= bounds.right);
    assert.ok(childBounds.top >= bounds.top);
    assert.ok(childBounds.bottom <= bounds.bottom);
  }
});

test("leaves horizontal space around wide expanded groups", () => {
  const wide = rectangle("wide", 0, 0, 180, 40, "group");
  const next = rectangle("next", 1, 0, 36, 40);

  layoutTimetableGraph(model([wide, next], []));

  assert.ok(
    timetableGraphNodeBounds(wide).right < timetableGraphNodeBounds(next).left,
  );
});

test("keeps adjacent expanded layer outlines separate", () => {
  const firstChild = rectangle("first-child", 10, 0, 60, 16, "group");
  const secondChild = rectangle("second-child", 11, 0, 60, 16, "group");
  const firstLayer = rectangle("first-layer", 10, 0, 50, 42, "layer");
  const secondLayer = rectangle("second-layer", 11, 0, 50, 42, "layer");
  firstLayer.expanded = true;
  secondLayer.expanded = true;
  firstLayer.layoutChildren = [firstChild];
  secondLayer.layoutChildren = [secondChild];
  firstChild.topNode = firstLayer;
  secondChild.topNode = secondLayer;
  firstChild.layoutAnchor = firstChild;
  secondChild.layoutAnchor = secondChild;

  layoutTimetableGraph(
    model(
      [firstLayer, firstChild, secondLayer, secondChild],
      [],
      [firstChild, secondChild],
    ),
  );

  assert.ok(
    timetableGraphNodeBounds(firstLayer).right <
      timetableGraphNodeBounds(secondLayer).left,
  );
});

test("separates layers whose ranked child columns are interleaved", () => {
  const firstStart = rectangle("first-start", 10, 0, 40, 16, "group");
  const firstEnd = rectangle("first-end", 12, 1, 40, 16, "group");
  const secondChild = rectangle("second-child", 11, 0, 40, 16, "group");
  const firstLayer = rectangle("first-layer", 10, 0, 50, 42, "layer");
  const secondLayer = rectangle("second-layer", 11, 0, 50, 42, "layer");
  firstLayer.expanded = true;
  secondLayer.expanded = true;
  firstLayer.layoutChildren = [firstStart, firstEnd];
  secondLayer.layoutChildren = [secondChild];
  for (const child of firstLayer.layoutChildren) {
    child.topNode = firstLayer;
    child.layoutAnchor = child;
  }
  secondChild.topNode = secondLayer;
  secondChild.layoutAnchor = secondChild;

  layoutTimetableGraph(
    model(
      [firstLayer, firstStart, firstEnd, secondLayer, secondChild],
      [],
      [firstStart, secondChild, firstEnd],
    ),
  );

  assert.ok(
    timetableGraphNodeBounds(firstLayer).right <
      timetableGraphNodeBounds(secondLayer).left,
  );
});

test("places a transfer tensor between its producer and consumer layers", () => {
  const producer = circle("producer", 0, 0);
  const tensor = rectangle("transfer", 1, 99, 12, 24);
  const consumer = circle("consumer", 1, 0);
  const producerLayer = rectangle("producer-layer", 0, 0, 50, 42, "layer");
  const consumerLayer = rectangle("consumer-layer", 1, 1, 50, 42, "layer");
  producerLayer.expanded = true;
  consumerLayer.expanded = true;
  producerLayer.layoutChildren = [producer];
  consumerLayer.layoutChildren = [consumer];
  producer.topNode = producerLayer;
  consumer.topNode = consumerLayer;
  const graph = model(
    [producerLayer, producer, tensor, consumerLayer, consumer],
    [link(producer, tensor), link(tensor, consumer)],
    [producer, tensor, consumer],
  );

  layoutTimetableGraph(graph);

  assert.ok(
    timetableGraphNodeBounds(producerLayer).right <
      timetableGraphNodeBounds(tensor).left,
  );
  assert.ok(
    timetableGraphNodeBounds(tensor).right <
      timetableGraphNodeBounds(consumerLayer).left,
  );
  for (const edge of graph.links) {
    assert.ok(edge.route.start.x < edge.route.end.x);
  }
});

test("keeps fly-by tensors aligned between their endpoint layers", () => {
  const producer = circle("producer", 0, 0);
  const firstTensor = rectangle("first-transfer", 1, 0, 12, 24);
  const secondTensor = rectangle("second-transfer", 1, 1, 12, 24);
  const consumer = circle("consumer", 3, 0);
  const producerLayer = rectangle("producer-layer", 0, 0, 50, 42, "layer");
  const consumerLayer = rectangle("consumer-layer", 3, 3, 50, 42, "layer");
  producerLayer.expanded = true;
  consumerLayer.expanded = true;
  producerLayer.layoutChildren = [producer];
  consumerLayer.layoutChildren = [consumer];
  producer.topNode = producerLayer;
  consumer.topNode = consumerLayer;
  firstTensor.bypassSide = "top";
  secondTensor.bypassSide = "bottom";
  const graph = model(
    [
      producerLayer,
      producer,
      firstTensor,
      secondTensor,
      consumerLayer,
      consumer,
    ],
    [
      link(producer, firstTensor),
      link(firstTensor, consumer),
      link(producer, secondTensor),
      link(secondTensor, consumer),
    ],
    [producer, firstTensor, secondTensor, consumer],
  );

  layoutTimetableGraph(graph);

  assert.equal(firstTensor.x, secondTensor.x);
  assert.ok(
    timetableGraphNodeBounds(producerLayer).right < firstTensor.x &&
      firstTensor.x < timetableGraphNodeBounds(consumerLayer).left,
  );
});

test("recomputes layer bounds before repacking a reused graph", () => {
  const firstChild = rectangle("first-child", 10, 0, 20, 16, "group");
  const secondChild = rectangle("second-child", 11, 0, 20, 16, "group");
  const firstLayer = rectangle("first-layer", 10, 0, 50, 42, "layer");
  const secondLayer = rectangle("second-layer", 11, 0, 50, 42, "layer");
  firstLayer.expanded = true;
  secondLayer.expanded = true;
  firstLayer.layoutChildren = [firstChild];
  secondLayer.layoutChildren = [secondChild];
  firstChild.topNode = firstLayer;
  secondChild.topNode = secondLayer;
  const graph = model(
    [firstLayer, firstChild, secondLayer, secondChild],
    [],
    [firstChild, secondChild],
  );

  layoutTimetableGraph(graph);
  firstChild.halfWidth = 200;
  layoutTimetableGraph(graph);

  assert.ok(
    timetableGraphNodeBounds(firstLayer).right <
      timetableGraphNodeBounds(secondLayer).left,
  );
});

test("ranks intra-layer tensors between their producer and consumer", () => {
  const producer = circle("producer", 0, 0);
  const tensor = rectangle("tensor", 0, 1, 9, 30);
  const consumer = circle("consumer", 0, 2);
  const layer = rectangle("layer", 0, 0, 50, 42, "layer");
  layer.expanded = true;
  layer.layoutChildren = [producer, tensor, consumer];
  for (const child of layer.layoutChildren) {
    child.topNode = layer;
  }

  layoutTimetableGraph(
    model(
      [layer, producer, tensor, consumer],
      [link(producer, tensor), link(tensor, consumer)],
      layer.layoutChildren,
    ),
  );

  assert.ok(producer.x < tensor.x && tensor.x < consumer.x);
  const layerBounds = timetableGraphNodeBounds(layer);
  for (const child of layer.layoutChildren) {
    const bounds = timetableGraphNodeBounds(child);
    assert.ok(bounds.left >= layerBounds.left);
    assert.ok(bounds.right <= layerBounds.right);
  }
});

test("keeps a locally consumed transfer tensor after its producer", () => {
  const producer = circle("producer", 0, 0);
  const tensor = rectangle("tensor", 0, 1, 9, 30);
  const localConsumer = circle("local-consumer", 0, 2);
  const laterConsumer = circle("later-consumer", 3, 0);
  const producerLayer = rectangle("producer-layer", 0, 0, 50, 42, "layer");
  const laterLayer = rectangle("later-layer", 3, 1, 50, 42, "layer");
  producerLayer.expanded = true;
  laterLayer.expanded = true;
  producerLayer.layoutChildren = [producer, tensor, localConsumer];
  laterLayer.layoutChildren = [laterConsumer];
  for (const child of producerLayer.layoutChildren)
    child.topNode = producerLayer;
  laterConsumer.topNode = laterLayer;

  layoutTimetableGraph(
    model(
      [
        producerLayer,
        producer,
        tensor,
        localConsumer,
        laterLayer,
        laterConsumer,
      ],
      [
        link(producer, tensor),
        link(tensor, localConsumer),
        link(tensor, laterConsumer),
      ],
      [producer, tensor, localConsumer, laterConsumer],
    ),
  );

  assert.ok(producer.x < tensor.x);
  assert.ok(tensor.x < localConsumer.x);
  assert.ok(tensor.x < laterConsumer.x);
  const layerBounds = timetableGraphNodeBounds(producerLayer);
  const tensorBounds = timetableGraphNodeBounds(tensor);
  assert.ok(tensorBounds.left >= layerBounds.left);
  assert.ok(tensorBounds.right <= layerBounds.right);
});

test("classifies tensors that belong inside one layer", () => {
  const layer0 = { layer: "Layer 0" };
  const layer1 = { layer: "Layer 1" };

  assert.deepEqual(
    timetableGraphTensorPlacement([
      { type: "write", compute: layer0 },
      { type: "read", compute: layer0 },
    ]),
    { layer: "Layer 0", role: "intra" },
  );
  assert.equal(
    timetableGraphTensorPlacement([
      { type: "write", compute: layer0 },
      { type: "read", compute: layer1 },
    ]),
    null,
  );
  assert.deepEqual(
    timetableGraphTensorPlacement([
      { type: "write", compute: layer0 },
      { type: "read", compute: layer0 },
      { type: "read", compute: layer1 },
    ]),
    { layer: "Layer 0", role: "transfer" },
  );
  assert.deepEqual(
    timetableGraphTensorPlacement([{ type: "read", compute: layer0 }]),
    { layer: "Layer 0", role: "input" },
  );
});

test("classifies tensor data-flow roles", () => {
  const read = { type: "read" };
  const write = { type: "write" };
  assert.equal(
    timetableGraphTensorRole([read], { role: "input" }, false),
    "layer-input",
  );
  assert.equal(
    timetableGraphTensorRole([read, write], { role: "intra" }, false),
    "intra-layer",
  );
  assert.equal(
    timetableGraphTensorRole([read, write], { role: "transfer" }, false),
    "layer-transfer",
  );
  assert.equal(
    timetableGraphTensorRole([read, write], null, true),
    "long-span",
  );
  assert.equal(timetableGraphTensorRole([read], null, false), "graph-input");
  assert.equal(timetableGraphTensorRole([write], null, false), "graph-output");
  assert.equal(
    timetableGraphTensorRole([read, write], null, false),
    "layer-transfer",
  );
  assert.equal(timetableGraphTensorRole([], null, false), "unconnected");
});

test("places pure layer inputs above and below the layer content", () => {
  const topInput = rectangle("top-input", 0, 0, 9, 20);
  topInput.layerInputSide = "top";
  const bottomInput = rectangle("bottom-input", 0, 1, 9, 24);
  bottomInput.layerInputSide = "bottom";
  const compute = circle("compute", 0, 2, 12);
  const layer = rectangle("layer", 0, 0, 50, 42, "layer");
  layer.expanded = true;
  layer.layoutChildren = [topInput, bottomInput, compute];
  for (const child of layer.layoutChildren) {
    child.topNode = layer;
  }

  layoutTimetableGraph(
    model(
      [layer, topInput, bottomInput, compute],
      [link(topInput, compute), link(bottomInput, compute)],
      layer.layoutChildren,
    ),
  );

  const computeBounds = timetableGraphNodeBounds(compute);
  assert.ok(timetableGraphNodeBounds(topInput).bottom < computeBounds.top);
  assert.ok(timetableGraphNodeBounds(bottomInput).top > computeBounds.bottom);
  const layerBounds = timetableGraphNodeBounds(layer);
  assert.ok(timetableGraphNodeBounds(topInput).top >= layerBounds.top);
  assert.ok(timetableGraphNodeBounds(bottomInput).bottom <= layerBounds.bottom);
});

test("identifies tensors read beyond the adjacent layer", () => {
  const indices = new Map([
    ["Layer 0", 0],
    ["Layer 1", 1],
    ["Layer 2", 2],
  ]);
  const index = (layer) => indices.get(layer);
  const write = { type: "write", compute: { layer: "Layer 0" } };

  assert.equal(
    timetableGraphTensorNeedsBypass(
      [write, { type: "read", compute: { layer: "Layer 1" } }],
      index,
    ),
    false,
  );
  assert.equal(
    timetableGraphTensorNeedsBypass(
      [write, { type: "read", compute: { layer: "Layer 2" } }],
      index,
    ),
    true,
  );
});

test("places long-lived tensors in an outer bypass lane", () => {
  const firstLayer = rectangle("layer-0", 0, 0, 42, 30, "layer");
  const middleLayer = rectangle("layer-1", 1_000, 1, 42, 46, "layer");
  const lastLayer = rectangle("layer-2", 2_000, 2, 42, 34, "layer");
  const tensor = rectangle("tensor", 999, 0, 18, 40);
  tensor.bypassSide = "top";
  const secondTensor = rectangle("second-tensor", 1_999, 1, 18, 20);
  secondTensor.bypassSide = "top";
  const write = link(firstLayer, tensor);
  write.type = "write";
  const read = link(tensor, lastLayer);

  layoutTimetableGraph(
    model(
      [firstLayer, middleLayer, lastLayer, tensor, secondTensor],
      [write, read],
    ),
  );

  const layerTop = Math.min(
    ...[firstLayer, middleLayer, lastLayer].map(
      (layer) => timetableGraphNodeBounds(layer).top,
    ),
  );
  assert.ok(timetableGraphNodeBounds(tensor).bottom < layerTop);
  assert.equal(secondTensor.y, tensor.y);
  assert.equal(write.route.control1.y, tensor.y);
  assert.equal(read.route.control2.y, tensor.y);
  assert.ok(write.route.start.x < write.route.end.x);
  assert.ok(read.route.start.x < read.route.end.x);
});

test("routes links from source boundary to target boundary", () => {
  const source = rectangle("source", 1, 0, 9, 60);
  const target = circle("target", 2, 0, 14);
  const edge = link(source, target);
  const graph = model([source, target], [edge]);

  layoutTimetableGraph(graph);

  assert.equal(edge.route.start.x, timetableGraphNodeBounds(source).right);
  assert.ok(edge.route.end.x < target.x);
  assert.ok(edge.route.end.x >= timetableGraphNodeBounds(target).left);
  const routeBounds = timetableGraphRouteBounds(edge.route);
  assert.ok(routeBounds.left <= edge.route.start.x);
  assert.ok(routeBounds.right >= edge.route.end.x);
});

test("separates dependencies that initially share a column", () => {
  const source = circle("source", 2, 0, 10);
  const target = circle("target", 2, 1, 10);
  const edge = link(source, target);

  layoutTimetableGraph(model([source, target], [edge]));

  assert.ok(source.x < target.x);
  assert.ok(edge.route.start.x < edge.route.end.x);
  assert.ok(edge.route.end.x < target.x);
});

test("spatial index returns nearby nodes without scanning the graph", () => {
  const near = circle("near", 0, 0);
  const far = circle("far", 8, 0);
  const graph = model([near, far], []);

  layoutTimetableGraph(graph);

  assert.ok(graph.spatialIndex.query(near.x, near.y).includes(near));
  assert.ok(graph.spatialIndex.query(far.x, far.y).includes(far));
});
