// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

function select_and_send(serverUrl, svg, id, data) {
  // Remove all currently selected nodes
  svg.selectAll(".selected").classed("selected", false);

  // Add the selected class to the selected node
  svg.selectAll(`#${id}`).classed("selected", true);

  // Select the node on the server
  console.log(`select #${data.id}`);
  d3.text(serverUrl + "/select/" + data.id)
    .then(function (text) {
      // console.log(text);
    })
    .catch(function (error) {
      console.log(error);
    });
}
