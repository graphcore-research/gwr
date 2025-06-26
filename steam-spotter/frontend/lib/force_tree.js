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
  const width = 1600;
  const height = 1200;

  const max_circle_r = 12;
  const min_circle_r = 3;
  const circle_r_range = max_circle_r - min_circle_r;

  // Convert the structure tree to a d3 hierarchy.
  const root = d3.hierarchy(data);
  const links = root.links();
  const nodes = root.descendants();

  // Create a map of tag -> node in the hierarchy.
  let entities_by_tag = new Map();
  nodes.forEach(function(node) {
    entities_by_tag.set(node.data.tag, node);

    // Compute a radius depending on depth in hierarchy.
    node.r = min_circle_r + (max_depth - node.data.depth) / max_depth * circle_r_range;
  });

  // Assign some properties to the links depending on their depth in the hierarchy.
  links.forEach(function(link) {
    const min_depth = Math.min(link.source.data.depth, link.target.data.depth);
    if (min_depth > 0) {
      link.strength = 2/(max_depth - min_depth);
    } else {
      link.strength = 0;
    }

    if (min_depth == 0) {
      // Make the top-level hierarchy links disappear.
      link.color = "#fff";
    } else {
      link.color = "#282";
    }

    // No arrow heads on the strcuture links.
    link.lineEnd = "";
  });

  // Add connections between ports that have different properties.
  for (const pair of connections) {
    const [from, to] = pair;
    links.push({
      source: entities_by_tag.get(from),
      target: entities_by_tag.get(to),
      strength: max_depth/5,
      color: "#000",
      lineEnd: "url(#arrow)"
    });
  }

  // Create the force simulation.
  const simulation = d3.forceSimulation(nodes)
      .force("link", d3.forceLink(links).id(d => d.id).distance(0).strength(d => d.strength))
      .force("charge", d3.forceManyBody().strength(-50))
      .force("x", d3.forceX())
      .force("y", d3.forceY())
      .force("collide", d3.forceCollide().radius(d => d.r + 4));

  // Add an SVG element to the #chart node of the HTML.
  const svg = d3.select("#chart")
      .append("svg")
      .attr("width", width)
      .attr("height", height)
      .attr("viewBox", [-width / 2, -height / 2, width, height])
      .attr("style", "max-width: 100%; height: auto;");

  define_arrow(svg);

  // Append links using the color and line ending defined above.
  const link = svg.append("g")
      .attr("stroke", "#999")
      .attr("stroke-opacity", 0.8)
    .selectAll("line")
    .data(links)
    .join("line")
      .attr("stroke", d => d.color)
      .attr("marker-end", d => d.lineEnd);

  // Function to create unique IDs for each element in order to be able to select them easily
  var uniqueId = 0;
  const unique = prefix => `${prefix}_${uniqueId++}`;

  // Append nodes with radius defined above.
  const node = svg.append("g")
      .attr("fill", "#fff")
      .attr("stroke", "#000")
      .attr("stroke-width", 1.5)
    .selectAll("circle")
    .data(nodes)
    .join("circle")
      .each(d => d.id = unique("node"))
      .attr("id", d => d.id)
      .attr("class", "node")
      .attr("fill", d => d.children ? null : "#000")
      .attr("stroke", d => d.children ? null : "#fff")
      .attr("r", d => d.r)
      .call(drag(simulation));

  node.append("title")
      .text(d => d.data.full_name);

  // When clicking on a block highlight it by setting the "selected" class
  node.on("click", function(d) {
      // Remove all currently selected nodes
      svg.selectAll(".selected").classed("selected", false);

      // Add the selected class to the selected node
      svg.selectAll(`#${d.target.id}`).classed("selected", true);

      // Reference to the original data element built up by 'parse_entities()'
      const data = d.target.__data__.data;

      // Select the node on the server
      d3.text(serverUrl + "/select/" + data.tag).then(function(text) {
        // console.log(text);
      })
      .catch(function(error) {
        console.log(error);
      });
  });

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

  return simulation;
}

function drag(simulation) {
  function dragstarted(event, d) {
    if (!event.active) simulation.alphaTarget(0.3).restart();
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
