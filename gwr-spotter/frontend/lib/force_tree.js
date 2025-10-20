// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

// Create the #arrow decoration that can be used on lines
function define_arrow(svg) {
  // Define the arrowhead marker variables.
  const markerBoxWidth = 10;
  const markerBoxHeight = 10;

  // Use refX to move the arrow back along the line.
  const refX = markerBoxWidth * 3/2;
  const refY = markerBoxHeight / 2;
  const arrowPoints = [[0, 0], [0, 10], [10, 5]];

  // Add the arrowhead marker definition to the svg element.
  svg
    .append('defs')
    .append('marker')
    .attr('id', 'arrow')
    .attr('viewBox', [0, 0, markerBoxWidth, markerBoxHeight])
    .attr('refX', refX)
    .attr('refY', refY)
    .attr('markerWidth', markerBoxWidth)
    .attr('markerHeight', markerBoxHeight)
    .attr('orient', 'auto-start-reverse')
    .append('path')
    .attr('d', d3.line()(arrowPoints))
    .attr('stroke', 'black');
}

//---------------------------------------------------------------------------------------
// From https://observablehq.com/@d3/force-directed-tree
function force_tree(serverUrl, data, connections, max_depth) {
  // Specify the chart’s dimensions.
  var chartDiv = document.getElementById(chartElement);
  var width = Math.max(600, chartDiv.clientWidth);
  var height = Math.max(400, chartDiv.clientHeight - buttonBarPadding);

  // Convert the structure tree to a d3 hierarchy.
  const root = d3.hierarchy(data);
  const nodes = root.descendants();
  let links = root.links();

  // Create a map of IDs -> node in the hierarchy.
  let entities_by_id = new Map();
  nodes.forEach(function(node) {
    entities_by_id.set(node.data.id, node);

    // Compute a radius depending on the number of children a node has.
    node.r = node.data.radius;

    // Assign the x,y to their pre-computed initial values.
    node.x = node.data.initial_x;
    node.y = node.data.initial_y;
  });

  // Remove the links between top and top-level nodes.
  links = links.filter(link => Math.min(link.source.data.depth, link.target.data.depth) > 0);

  // Assign some properties to the links depending on their depth in the hierarchy.
  links.forEach(function(link) {
    const min_depth = Math.min(link.source.data.depth, link.target.data.depth);
    link.strength = 2 / (max_depth - min_depth);
    link.color = "#282";

    // No arrow heads on the strcuture links.
    link.lineEnd = "";
  });

  // Add connections between ports that have different properties.
  for (const pair of connections) {
    const [from, to] = pair;
    links.push({
      source: entities_by_id.get(from),
      target: entities_by_id.get(to),
      strength: max_depth/5,
      color: "#000",
      lineEnd: "url(#arrow)"
    });
  }

  // Create the force simulation.
  const simulation = d3.forceSimulation(nodes)
      .force("link", d3.forceLink(links).id(d => d.id).distance(0).strength(d => d.strength))
      .force("charge", d3.forceManyBody().strength(-30))
      .force("x", d3.forceX().strength(0.1))
      .force("y", d3.forceY().strength(0.1))
      .alphaDecay(0.005);

  // Add an SVG element to the chart node of the HTML.
  const svg = d3.select(`#${chartElement}`)
      .append("svg")
      .attr("width", width)
      .attr("height", height)
      .attr("viewBox", [-width / 2, -height / 2, width, height])
      .attr("style", "max-width: 100%; height: auto;");

  define_arrow(svg);

  // Append nodes with radius defined above.
  const node = svg.append("g")
      .attr("fill", "#fff")
      .attr("stroke", "#000")
      .attr("stroke-width", 1.5)
    .selectAll("circle")
    .data(nodes)
    .join("circle")
      .each(d => d.id = `node_${d.data.id}`)
      .attr("id", d => d.id)
      .attr("class", "node")
      .attr("fill", d => d.children ? null : "#000")
      .attr("stroke", d => d.children ? null : "#fff")
      .attr("opacity", d => d.children ? 0.5 : 1)
      .attr("r", d => d.r)
      .call(drag(simulation));

  node.append("title")
      .text(d => d.data.full_name);

  // When clicking on a block highlight it by setting the "selected" class
  node.on("click", d => select_and_send(serverUrl, svg, d.target.id, d.target.__data__.data.id));

  // Append links using the color and line ending defined above.
  const link = svg.append("g")
      .attr("stroke", "#999")
      .attr("stroke-opacity", 0.8)
    .selectAll("line")
    .data(links)
    .join("line")
      .attr("stroke", d => d.color)
      .attr("marker-end", d => d.lineEnd);

  simulation.on("tick", () => {
    link
        .attr("x1", d => d.source.x)
        .attr("y1", d => d.source.y)
        .attr("x2", d => d.target.x)
        .attr("y2", d => d.target.y);

    node
        .attr("cx", d => d.x)
        .attr("cy", d => d.y);
  });

  get_selected(serverUrl, svg);
  return simulation;
}

function drag(simulation) {
  function dragstarted(event, d) {
    if (!event.active) simulation.alphaTarget(0.8).restart();
    d.fx = d.x;
    d.fy = d.y;
  }

  function dragged(event, d) {
    d.fx = event.x;
    d.fy = event.y;
  }

  function dragended(event, d) {
    if (!event.active) simulation.alphaTarget(0);
    d.fx = null;
    d.fy = null;
  }

  return d3.drag()
    .on("start", dragstarted)
    .on("drag", dragged)
    .on("end", dragended);
}
