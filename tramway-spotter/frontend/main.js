// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

// Add an entity within a hierarchical tree of nodes
function add_entity(depth, nodes, path_remaining, full_name, id) {
  if (path_remaining.length == 0) {
    return nodes;
  }

  const name = path_remaining[0];
  path_remaining = path_remaining.slice(1);

  let node = null;
  for (const n of nodes) {
    if (n.name == name) {
      node = n;
      break;
    }
  }

  if (node == null) {
    node = {
      name: name,
      full_name: full_name,
      depth: depth,
      id: id,
    }
    nodes.push(node);
  }

  if (Object.hasOwn(node, "children")) {
    children = node.children;
  } else {
    children = [];
  }
  children = add_entity(depth + 1, children, path_remaining, full_name, id);

  if (children.length > 0) {
    node.children = children;
  } else {
    node.value = 1;
    node.children = [];
  }
  return nodes;
}

// Annotate each node with the number of children it has
//
// This is used by the force_tree to determine the size of nodes such that
// they roughly include their children.
function annotate_num_children(node) {
  num_children = node.children.length;
  node.children.forEach(function(child, index) {
    num_children += annotate_num_children(child);
  });
  node.num_children = num_children;
  return num_children;
}

// Assign initial positions to nodes for the force_tree.
//
// This attempts to use a grid pattern taking assigning nodes at each level of hierarchy in
// a pattern such that if there were 10 children it would be laid out as:
// 0 1 2 3
// 7 6 5 4
// 8 9
// with the assumption that neighbouring nodes will be connected.
// For each node, children nodes are laid out in the same pattern in the area allocated to
// that node.
function set_initial_xy(node, min_x, max_x, min_y, max_y) {
  let num_children = node.children.length;
  if (num_children > 0) {
    let num_per_row = Math.ceil(Math.sqrt(num_children));
    let num_rows = Math.ceil(num_children / num_per_row);
    let x_step = (max_x - min_x) / num_per_row;
    let y_step = (max_y - min_y) / num_rows;
    node.children.forEach(function(child, i) {
      let row = Math.floor(i / num_per_row);
      let odd_row = row & 0x1;
      if (odd_row) {
        // Work from right to left
        child.initial_x = max_x - (min_x + (i % num_per_row) * x_step);
      } else {
        // Work from left to right
        child.initial_x = min_x + (i % num_per_row) * x_step;
      }
      child.initial_y = min_y + Math.floor(i / num_per_row) * y_step;
      set_initial_xy(child, child.initial_x, child.initial_x + x_step, child.initial_y, child.initial_y + y_step);
    });
  }
}

// Parse create messages of the form:
//   hierarchical::name=id
function parse_entities(text, entities) {
  const lines = text.split("\n")
  let max_depth = 0;
  lines.forEach(function(line) {
    const [name, id] = line.split("=");
    const name_path = name.split("::");
    max_depth = Math.max(max_depth, name_path.length);
    add_entity(0, entities, name_path, name, id);
  });

  annotate_num_children(entities[0]);
  let max_xy = entities[0].num_children * 10;
  set_initial_xy(entities[0], 0, max_xy, 0, max_xy);
  return max_depth - 1;
}

function add_connection(connections, line_data) {
  if (line_data.length != 2) {
    console.log("Ignoring line:");
    console.log(line_data);
  }
  const from_id = line_data[0].trim();
  const to_id = line_data[1].trim();
  connections.push([from_id, to_id]);
}

// Parse connect messages of the form:
//  from_id->to_id
function parse_connections(text) {
  let connections = [];
  const lines = text.split("\n")
  lines.forEach(function(line) {
    add_connection(connections, line.split("->"));
  });
  return connections;
}

//---------------------------------------------------------------------------------------
// From https://github.com/observablehq/stdlib/blob/main/src/dom/uid.js
var count = 0;

function uid(name) {
  return new Id("O-" + (name == null ? "" : name + "-") + ++count);
}

function Id(id) {
  this.id = id;
  this.href = new URL(`#${id}`, location) + "";
}

Id.prototype.toString = function() {
  return "url(" + this.href + ")";
};


class Renderer {
  constructor(serverUrl) {
    this.serverUrl = serverUrl;
    this.guiElements = this._createGuiElements();
    this.controls = this._createDefaultControls();

    this.entities = null;
    this.max_depth = 0;
    this.connections = null;
    this.simulation = null;
  }

  /**
   * Create the default control values that can be either set on the command
   * line or through GUI elements
   */
  _createDefaultControls() {
    var controls = {
      plot : { url: 'plot', value: 'force_tree' },
    };

    // Set the default value to the current value
    Object.keys(controls).forEach(function(key) {
      const control = controls[key];
      control.default = control.value;
    });
    return controls;
  }

    /**
   * Create the graphical elements of the GUI (e.g. x-axis, y-axis)
   */
  _createGuiElements() {
    const plotTypes = new Map()
        .set('force_tree', force_tree)
        .set('tree_map', tree_map)
        .set('sunburst', sunburst)
        .set('radial_tidy_tree', radial_tidy_tree);

    return {
      plotTypes: plotTypes
    };
  }

  set_entities(entities, max_depth) {
    this.entities = entities;
    this.max_depth = max_depth;
  }

  set_connections(connections) {
    this.connections = connections;
  }

  render() {
    if (this.entities == null) {
      console.log("Nothing to render");
      return;
    }

    // Remove any current render
    d3.select("#chart").selectAll("svg").remove();
    if (this.simulation != null) {
      this.simulation.stop();
      this.simulation = null;
    }

    const controls = this.controls;
    const guiElements = this.guiElements;
    const plot = guiElements.plotTypes.get(controls.plot.value);
    this.simulation = plot(this.serverUrl, this.entities, this.connections, this.max_depth);
  }
}
