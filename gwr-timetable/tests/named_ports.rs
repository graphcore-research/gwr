// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::HashMap;

use gwr_models::processing_element::tensors::HasShape;
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_timetable::graph::TimetableGraph;
use gwr_timetable::mermaid::render_mermaid;
use gwr_timetable::timetable_file::TimetableFile;

const NAMED: &str = r"
nodes:
  - { id: left, kind: tensor, config: { addr: 0, dtype: fp32, shape: [8] } }
  - { id: right, kind: tensor, config: { addr: 128, dtype: fp32, shape: [4] } }
  - { id: result, kind: tensor, config: { addr: 256, dtype: fp32, shape: [8] } }
  - id: add
    kind: compute
    program:
      inputs: [A, B]
      outputs: [Y]
      assignments:
        - output: Y
          axes: [i]
          expr:
            add:
              - read: { tensor: A, indices: [i] }
              - read: { tensor: B, indices: [i] }
    input_views:
      A: { offsets: [2], shape: [4] }
    output_views:
      Y: { offsets: [4], shape: [4] }
edges:
  - { from: right, to: add.B, kind: data }
  - { from: add.Y, to: result, kind: data }
  - { from: left, to: add.A, kind: data }
";

fn error(source: &str) -> String {
    TimetableFile::from_string(source)
        .and_then(TimetableFile::into_graph)
        .unwrap_err()
        .to_string()
}

fn named_graph(inputs: &str) -> TimetableGraph {
    TimetableFile::from_string(&NAMED.replace("[A, B]", inputs))
        .unwrap()
        .into_graph()
        .unwrap()
}

#[test]
fn named_bindings_and_sparse_views_follow_declaration_identity() {
    for (inputs, expected) in [("[A, B]", [2, 0]), ("[B, A]", [0, 2])] {
        let graph = named_graph(inputs);
        let views = graph.compute_views(3).unwrap();
        assert_eq!(views.inputs()[0].offsets(), &[expected[0]]);
        assert_eq!(views.inputs()[1].offsets(), &[expected[1]]);
        assert_eq!(views.outputs()[0].offsets(), &[4]);
        assert_eq!(views.inputs()[0].shape().dims(), &[4]);
        assert_eq!(graph.nodes()[3].predecessors(), &[1, 0]);
        assert_eq!(graph.nodes()[3].successors(), &[2]);
    }
}

#[test]
fn mermaid_labels_preserve_named_sparse_views() {
    for inputs in ["[A, B]", "[B, A]"] {
        let graph = named_graph(inputs);
        let mermaid = render_mermaid(&graph, &HashMap::new());
        assert!(mermaid.contains("A: 4 @ 2"), "{mermaid}");
        assert!(mermaid.contains("Y: 4 @ 4"), "{mermaid}");
    }
}

#[test]
fn named_endpoint_helpers_return_port_names() {
    let file = TimetableFile::from_string(NAMED).unwrap();
    assert_eq!(
        file.edges[0].to_node_and_port().unwrap(),
        ("add", Some("B"))
    );
    let graph = file.clone().into_graph().unwrap();
    assert_eq!(graph.edges()[0].to_port_name(&graph), Some("B"));
    assert_eq!(graph.edges()[0].from_port_name(&graph), None);
    assert_eq!(graph.edges()[1].from_port_name(&graph), Some("Y"));
    assert_eq!(graph.edges()[1].to_port_name(&graph), None);
    assert_eq!(file.edges[1].from_node_id(), "add");
}

#[test]
fn unknown_wrong_direction_missing_and_duplicate_bindings_are_diagnosed() {
    for (old, new, expected) in [
        ("to: add.B", "to: add.typo", "unknown input port 'typo'"),
        ("to: add.B", "to: add..B", "unknown input port '.B'"),
        ("to: add.B", "to: add.Y", "unknown input port 'Y'"),
        ("from: add.Y", "from: add.A", "unknown output port 'A'"),
        ("to: add.B", "to: add", "requires a named input port"),
        (
            "to: add.A",
            "to: add.B",
            "input port 'B' is connected more than once",
        ),
        (
            "inputs: [A, B]",
            "inputs: [A, B, C]",
            "unbound program input 'C'",
        ),
        (
            "inputs: [A, B]",
            "inputs: [A, A]",
            "duplicate program port 'A'",
        ),
        ("outputs: [Y]", "outputs: [A]", "duplicate program port 'A'"),
    ] {
        let actual = error(&NAMED.replace(old, new));
        assert!(actual.contains(expected), "{expected}: {actual}");
    }
}

#[test]
fn unknown_wrong_direction_and_duplicate_views_are_diagnosed() {
    for (old, new, expected) in [
        (
            "A: { offsets",
            "typo: { offsets",
            "unknown input view port 'typo'",
        ),
        (
            "A: { offsets",
            "Y: { offsets",
            "unknown input view port 'Y'",
        ),
        (
            "Y: { offsets",
            "A: { offsets",
            "unknown output view port 'A'",
        ),
        (
            "A: { offsets: [2], shape: [4] }",
            "A: { offsets: [2], shape: [4] }\n      A: { offsets: [0], shape: [4] }",
            "duplicate view port 'A'",
        ),
    ] {
        let actual = error(&NAMED.replace(old, new));
        assert!(actual.contains(expected), "{expected}: {actual}");
    }
}

#[test]
fn identifiers_reject_empty_names_and_ambiguous_node_delimiters() {
    for (old, new) in [
        ("id: add", "id: 'add.part'"),
        ("id: add", "id: ''"),
        ("inputs: [A, B]", "inputs: ['', B]"),
        ("to: add.B", "to: add."),
        ("to: add.B", "to: .B"),
    ] {
        let actual = error(&NAMED.replace(old, new));
        assert!(
            actual.contains("identifier") || actual.contains("endpoint"),
            "{actual}"
        );
    }
}

#[test]
fn named_views_and_edges_roundtrip_deterministically() {
    let source = NAMED.replace(
        "    input_views:\n",
        "    input_views:\n      B: { offsets: [0], shape: [4] }\n",
    );
    let file = TimetableFile::from_string(&source).unwrap();
    let serialized = yaml_serde::to_string(&file).unwrap();
    let restored = TimetableFile::from_string(&serialized).unwrap();
    restored.validate().unwrap();
    assert_eq!(serialized, yaml_serde::to_string(&restored).unwrap());
    assert!(serialized.find("    A:").unwrap() < serialized.find("    B:").unwrap());
    assert_eq!(restored.edges[0].to, "add.B");
    assert_eq!(restored.edges[1].from, "add.Y");
}

#[test]
fn scalar_named_views_keep_empty_shape_and_offsets() {
    let source = NAMED
        .replace("shape: [8]", "shape: []")
        .replace("shape: [4]", "shape: []")
        .replace("offsets: [2]", "offsets: []")
        .replace("offsets: [4]", "offsets: []")
        .replace("axes: [i]", "axes: []")
        .replace("indices: [i]", "indices: []");
    let graph = TimetableFile::from_string(&source)
        .unwrap()
        .into_graph()
        .unwrap();
    let views = graph.compute_views(3).unwrap();
    assert!(views.inputs()[0].shape().dims().is_empty());
    assert!(views.outputs()[0].offsets().is_empty());
}

#[test]
fn named_multi_output_reduction_keeps_value_and_index_dtypes_when_reordered() {
    let source = r"
nodes:
  - { id: input, kind: tensor, config: { addr: 0, dtype: fp32, shape: [4] } }
  - { id: value, kind: tensor, config: { addr: 64, dtype: fp32, shape: [] } }
  - { id: index, kind: tensor, config: { addr: 128, dtype: int64, shape: [] } }
  - id: maximum
    kind: compute
    program:
      inputs: [A]
      outputs: [I, Y]
      assignments:
        - output: Y
          indices_output: I
          axes: []
          expr:
            reduce:
              op: max
              over: [k]
              value:
                read: { tensor: A, indices: [k] }
    output_views:
      I: { offsets: [], shape: [] }
edges:
  - { from: input, to: maximum.A, kind: data }
  - { from: maximum.Y, to: value, kind: data }
  - { from: maximum.I, to: index, kind: data }
";
    let graph = TimetableFile::from_string(source)
        .unwrap()
        .into_graph()
        .unwrap();
    let views = graph.compute_views(3).unwrap();
    assert_eq!(views.outputs()[0].dtype(), &DataType::Int64);
    assert_eq!(views.outputs()[1].dtype(), &DataType::Fp32);
    assert_eq!(graph.nodes()[3].output_edges(), &[2, 1]);
}

#[test]
fn quoted_numeric_unicode_nodes_and_dotted_ports_bind_by_exact_name() {
    let source = NAMED
        .replace("id: left", "id: '123'")
        .replace("from: left", "from: '123'")
        .replace("id: add", "id: Δ")
        .replace("[A, B]", "[A.part, '0']")
        .replace("[Y]", "[Y.Ω]")
        .replace("output: Y", "output: Y.Ω")
        .replace("tensor: A,", "tensor: A.part,")
        .replace("tensor: B,", "tensor: '0',")
        .replace("A: {", "A.part: {")
        .replace("Y: {", "Y.Ω: {")
        .replace("add.A", "Δ.A.part")
        .replace("add.B", "Δ.0")
        .replace("add.Y", "Δ.Y.Ω");
    let file = TimetableFile::from_string(&source).unwrap();
    assert_eq!(file.edges[0].to_node_and_port().unwrap(), ("Δ", Some("0")));
    assert_eq!(
        file.edges[1].from_node_and_port().unwrap(),
        ("Δ", Some("Y.Ω"))
    );
    let graph = file.into_graph().unwrap();
    let views = graph.compute_views(3).unwrap();
    assert_eq!(views.inputs()[0].offsets(), &[2]);
    assert_eq!(views.inputs()[1].offsets(), &[0]);
    assert_eq!(views.outputs()[0].offsets(), &[4]);
}
