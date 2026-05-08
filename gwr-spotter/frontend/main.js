// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

// Add an entity within a hierarchical tree of nodes
function add_entity(depth, nodes, path_remaining, full_name, id, capacities) {
  if (path_remaining.length == 0) {
    return nodes;
  }

  const is_leaf = path_remaining.length == 1;
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
      id: is_leaf ? id : undefined,
      capacity: 0,
      capacity_units: "",
    }
    nodes.push(node);
  }

  if (is_leaf && id != undefined) {
    node.id = id;
  }

  if (is_leaf && id != undefined && capacities.has(id)) {
    const capacity = capacities.get(id);
    node.capacity = capacity.value;
    node.capacity_units = capacity.units;
  }

  if (Object.hasOwn(node, "children")) {
    children = node.children;
  } else {
    children = [];
  }
  children = add_entity(depth + 1, children, path_remaining, full_name, id, capacities);

  if (children.length > 0) {
    node.children = children;
  } else {
    node.value = 1;
    node.children = [];
  }
  return nodes;
}

// Annotate each node with the size of circle it should be
//
// This is used by the force_tree to determine the size of nodes such that
// they roughly include their children.
function annotate_radius(node) {
  let area = 0;
  node.children.forEach(function(child, index) {
    area += annotate_radius(child);
  });

  if (area == 0) {
    // Size of smallest node
    node.radius = 3;
  } else {
    // Allow for a 10% overhead
    node.radius = Math.sqrt(area) * 1.1;
  }

  // Allow a square box for each entity
  return Math.pow(2 * node.radius, 2);
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
function parse_entities(text, entities, capacities) {
  const lines = text.split("\n");
  let max_depth = 0;
  lines.forEach(function(line) {
    if (line.trim().length == 0) {
      return;
    }
    const [name, id] = line.split("=");
    const name_path = name.split("::");
    max_depth = Math.max(max_depth, name_path.length);
    add_entity(0, entities, name_path, name, id, capacities);
  });

  annotate_num_children(entities[0]);
  annotate_radius(entities[0]);
  let max_xy = entities[0].num_children * 10;
  set_initial_xy(entities[0], 0, max_xy, 0, max_xy);
  return max_depth - 1;
}

// Parse capacity messages of the form:
//   id=capacity,units
// Older servers may still send:
//   id=capacity
function parse_capacities(text) {
  let capacities = new Map();
  const lines = text.split("\n");
  lines.forEach(function(line) {
    if (line.trim().length == 0) {
      return;
    }
    const [id, capacity_str] = line.split("=");
    const [value_str, units = ""] = capacity_str.split(",");
    const value = Number(value_str);
    if (!Number.isNaN(value)) {
      capacities.set(id.trim(), {
        value: value,
        units: units.trim(),
      });
    }
  });
  return capacities;
}

// Parse fullness messages of the form:
//   id=fullness
function parse_fullnesses(text) {
  let fullnesses = new Map();
  const lines = text.split("\n");
  lines.forEach(function(line) {
    if (line.trim().length == 0) {
      return;
    }
    const [id, fullness_str] = line.split("=");
    const fullness = Number(fullness_str);
    if (!Number.isNaN(fullness)) {
      fullnesses.set(id.trim(), fullness);
    }
  });
  return fullnesses;
}

function apply_fullnesses(node, fullnesses) {
  let fullness = 0;
  if (node.id != undefined) {
    fullness += fullnesses.get(String(node.id)) || 0;
  }

  (node.children || []).forEach(function(child) {
    fullness += apply_fullnesses(child, fullnesses);
  });

  node.fullness = fullness;
  return fullness;
}

function fullness_ratio(data, capacity) {
  if (capacity <= 0) {
    return 0.0;
  }

  return Math.min(1.0, Math.max(0.0, (data.fullness || 0) / capacity));
}

function fullness_color(ratio) {
  return d3.scaleLinear()
      .domain([0.0, 0.5, 1.0])
      .range(["#35a853", "#f2c14e", "#d64545"])
      .clamp(true)(ratio);
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
    if (line.trim().length == 0) {
      return;
    }
    add_connection(connections, line.split("->"));
  });
  return connections;
}

// Parse position messages of the form:
//   line=123
//   lines=456
//   time=78.9
function parse_position(text) {
  const position = {
    line: 0,
    lines: 0,
    time: 0.0,
  };

  text.split("\n").forEach(function(line) {
    const [key, value] = line.split("=");
    if (key == "line") {
      position.line = Number(value);
    } else if (key == "lines") {
      position.lines = Number(value);
    } else if (key == "time") {
      position.time = Number(value);
    }
  });

  if (Number.isNaN(position.line)) {
    position.line = 0;
  }
  if (Number.isNaN(position.lines)) {
    position.lines = 0;
  }
  if (Number.isNaN(position.time)) {
    position.time = 0.0;
  }
  return position;
}

//---------------------------------------------------------------------------------------
function href(id) {
  return new URL(`#${id}`, location) + "";
}

function url(id) {
  return "url(" + href(id) + ")";
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
    this.positionSlider = null;
    this.positionLabel = null;
    this.positionPoll = null;
    this.positionSeekTimer = null;
    this.positionUserActive = false;
    this.pendingSeekLine = null;
    this.pendingSeekStartedAt = 0;
    this.pendingSeekTimeoutMs = 2000;
    this.fullnesses = new Map();
    this.lastPosition = {
      line: 0,
      lines: 0,
      time: 0.0,
    };
  }

  /**
   * Create the default control values that can be either set on the command
   * line or through GUI elements
   */
  _createDefaultControls() {
    var controls = {
      plot : { url: 'plot', value: 'sunburst' },
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
        .set('sunburst', sunburst)
        .set('force_tree', force_tree)
        .set('tree_map', tree_map)
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

  set_fullnesses(fullnesses) {
    this.fullnesses = fullnesses;
    if (this.entities != null) {
      apply_fullnesses(this.entities, fullnesses);
      if (this.controls.plot.value == "tree_map") {
        update_tree_map_fullness(this.entities);
      } else if (this.controls.plot.value == "force_tree") {
        update_force_tree_fullness(this.entities);
      }
    }
  }

  set_position_controls(slider, label) {
    this.positionSlider = slider;
    this.positionLabel = label;

    const renderer = this;
    slider.on("input", function() {
      renderer.positionUserActive = true;
      const line = Number(slider.property("value"));
      renderer._render_position({
        line: line,
        lines: renderer.lastPosition.lines,
        time: renderer.lastPosition.time,
      });
      renderer._debounced_seek(line);
    });
    slider.on("change", function() {
      const line = Number(slider.property("value"));
      renderer._seek(line);
      renderer.positionUserActive = false;
    });
    slider.on("pointerdown", function() {
      renderer.positionUserActive = true;
    });
    slider.on("pointerup", function() {
      const line = Number(slider.property("value"));
      renderer._seek(line);
      renderer.positionUserActive = false;
    });
  }

  start_position_sync() {
    if (this.positionPoll != null) {
      return;
    }

    this._poll_position();
    this.positionPoll = setInterval(() => this._poll_position(), 500);
  }

  _poll_position() {
    if (this.positionSlider == null) {
      return;
    }

    Promise.all([
      d3.text(this.serverUrl + "/position"),
      d3.text(this.serverUrl + "/fullnesses"),
    ])
        .then(([positionText, fullnessText]) => {
          const position = parse_position(positionText);
          if (!this._server_position_is_current(position)) {
            return;
          }

          this.lastPosition = position;
          this.set_fullnesses(parse_fullnesses(fullnessText));
          if (!this.positionUserActive) {
            this._render_position(position);
          }
        })
        .catch(function(error) {
          console.log(error);
        });
  }

  _server_position_is_current(position) {
    if (this.pendingSeekLine == null) {
      return true;
    }

    if (position.line == this.pendingSeekLine) {
      this.pendingSeekLine = null;
      return true;
    }

    // If something went wrong or the TUI clamped differently, eventually trust
    // the server again rather than freezing the control forever.
    if (Date.now() - this.pendingSeekStartedAt > this.pendingSeekTimeoutMs) {
      this.pendingSeekLine = null;
      return true;
    }

    return false;
  }

  _render_position(position) {
    if (this.positionSlider == null || this.positionLabel == null) {
      return;
    }

    const maxLine = Math.max(0, position.lines);
    const line = Math.min(Math.max(0, position.line), maxLine);
    this.positionSlider
        .attr("min", maxLine > 0 ? 1 : 0)
        .attr("max", maxLine)
        .property("value", line);
    this.positionLabel.text(`${line} / ${maxLine} @ ${position.time.toFixed(1)}ns`);
  }

  _debounced_seek(line) {
    clearTimeout(this.positionSeekTimer);
    this.positionSeekTimer = setTimeout(() => this._seek(line), 80);
  }

  _seek(line) {
    if (Number.isNaN(line)) {
      return;
    }

    this.pendingSeekLine = line;
    this.pendingSeekStartedAt = Date.now();

    d3.text(this.serverUrl + `/seek/${line}`)
        .catch(function(error) {
          console.log(error);
        });
  }

  render() {
    if (this.entities == null) {
      console.log("Nothing to render");
      return;
    }

    // Remove any current render
    d3.select(`#${chartElement}`).selectAll("svg").remove();
    if (this.simulation != null) {
      this.simulation.stop();
      this.simulation = null;
    }

    const controls = this.controls;
    const guiElements = this.guiElements;
    const plot = guiElements.plotTypes.get(controls.plot.value);
    apply_fullnesses(this.entities, this.fullnesses);
    this.simulation = plot(this.serverUrl, this.entities, this.connections, this.max_depth);
  }
}
