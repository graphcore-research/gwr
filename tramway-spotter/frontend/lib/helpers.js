// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

function select(svg, id, data, d) {
  // Remove all currently selected nodes
  svg.selectAll(".selected").classed("selected", false);

  // Add the selected class to the selected node
  console.log(`select #${id}`);
  svg.selectAll(`#${id}`).classed("selected", true);

  // Select the node on the server
  d3.text(serverUrl + "/select/" + data.id).then(function (text) {
    // console.log(text);
  })
    .catch(function (error) {
      console.log(error);
    });
}
