// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

((app, d3) => {
  function select_and_send(serverUrl, svg, html_id, node_id) {
    select(svg, html_id);
    app.sendSelect(serverUrl, node_id);
  }

  function select(svg, html_id) {
    // Remove all currently selected nodes
    svg.selectAll(".selected").classed("selected", false);

    // Add the selected class to the selected node
    svg.selectAll(`#${html_id}`).classed("selected", true);
  }

  function send_select(serverUrl, node_id) {
    // Select the node on the server
    console.log(`select ${node_id}`);
    d3.text(serverUrl + "/select/" + node_id).catch(function (error) {
      console.log(error);
    });
  }

  // Get the ID of the selected node
  function get_selected(serverUrl, svg) {
    d3.text(serverUrl + "/selected")
      .then(function (text) {
        const words = text.split(" ");
        if (words[1] == "selected") {
          const node_id = Number(words[0]);
          select(svg, `node_${node_id}`);
        }
      })
      .catch(function (error) {
        console.log(error);
      });
  }

  Object.assign(app, {
    getSelected: get_selected,
    selectAndSend: select_and_send,
    sendSelect: send_select,
  });
})(window.GWR_SPOTTER, window.d3);
