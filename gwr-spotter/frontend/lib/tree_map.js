// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//---------------------------------------------------------------------------------------
// From https://observablehq.com/@d3/nested-treemap
//
// Copyright 2019–2023 Observable, Inc.
//
// Permission to use, copy, modify, and/or distribute this software for any
// purpose with or without fee is hereby granted, provided that the above
// copyright notice and this permission notice appear in all copies.
//
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
// WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
// MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
// ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
// WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
// ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
// OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
function tree_map(serverUrl, data) {
  var chartDiv = document.getElementById(chartElement);
  var width = Math.max(600, chartDiv.clientWidth);
  var height = Math.max(400, chartDiv.clientHeight - buttonBarPadding);

  const color = d3.scaleSequential([0, 8], d3.interpolateGnBu);

  // Create the treemap layout.
  const treemap = data => d3.treemap()
    .size([width, height])
    .paddingOuter(3)
    .paddingTop(19)
    .paddingInner(1)
    .round(true)
  (d3.hierarchy(data)
      .sum(d => d.value)
      .sort((a, b) => b.value - a.value));
  const root = treemap(data);

  // Create the SVG container.
  const svg = d3.select(`#${chartElement}`)
      .append("svg")
      .attr("width", width)
      .attr("height", height)
      .attr("viewBox", [0, 0, width, height])
      .attr("style", "max-width: 100%; height: auto; overflow: visible; font: 10px sans-serif;");

  // Unique ID for shadow
  const shadow = "shadow-0";

  svg.append("filter")
      .attr("id", shadow)
    .append("feDropShadow")
      .attr("flood-opacity", 0.3)
      .attr("dx", 0)
      .attr("stdDeviation", 3);

  const node = svg.selectAll("g")
    .data(d3.group(root, d => d.height))
    .join("g")
      .attr("filter", url(shadow))
    .selectAll("g")
    .data(d => d[1])
    .join("g")
      .attr("transform", d => `translate(${d.x0},${d.y0})`)
      .on("click", (event, d) => select_and_send(serverUrl, svg, d.id, d.data.id));

  const format = d3.format(",d");
  node.append("title")
      .text(d => d.data.full_name);

  node.append("rect")
      .attr("id", d => (d.id = `node_${d.data.id}`))
      .attr("class", "node")
      .attr("fill", d => color(d.height))
      .attr("width", d => d.x1 - d.x0)
      .attr("height", d => d.y1 - d.y0);

  node.append("clipPath")
      .attr("id", d => (d.clipUid = `clip_${d.data.id}`))
    .append("use")
      .attr("xlink:href", d => href(d.clipUid));

  node.append("text")
      .attr("clip-path", d => url(d.clipUid))
    .selectAll("tspan")
    .data(d => d.data.name.split(/(?=[A-Z][^A-Z])/g).concat(format(d.value)))
    .join("tspan")
      .attr("fill-opacity", (d, i, nodes) => i === nodes.length - 1 ? 0.7 : null)
      .text(d => d);

  node.filter(d => d.children).selectAll("tspan")
      .attr("dx", 3)
      .attr("y", 13);

  node.filter(d => !d.children).selectAll("tspan")
      .attr("x", 3)
      .attr("y", (d, i, nodes) => `${(i === nodes.length - 1) * 0.3 + 1.1 + i * 0.9}em`);

  get_selected(serverUrl, svg);
}
