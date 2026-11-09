// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
use gwr_models::processing_element::program::syntax::{LocalRoundPolicy, ProgramSpec};
use gwr_timetable::timetable_file::{ProgramDefinition, TimetableFile};
fn plain() -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(include_str!("data/local-round-timetable.json"))
        .unwrap()["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["kind"] == "compute")
        .unwrap()["program"]
        .clone()
}
#[test]
fn exact_source_policy_carriers_preserve_omission_and_conversion() {
    let value = plain();
    let old: ProgramSpec = serde_json::from_value(value.clone()).unwrap();
    assert!(
        serde_json::to_value(&old)
            .unwrap()
            .get("local_round_policy")
            .is_none()
    );
    // Compare the same carrier serialization boundary on the None baseline:
    // ordinary ReadSpec already emits its require_in_range default explicitly.
    let baseline = serde_json::to_value(ProgramDefinition::from(old)).unwrap();
    assert!(baseline.get("local_round_policy").is_none());
    let mut selected = value.clone();
    selected["local_round_policy"] = "v41_round_scale_scalar".into();
    let program: ProgramSpec = serde_json::from_value(selected.clone()).unwrap();
    assert_eq!(
        program.local_round_policy,
        Some(LocalRoundPolicy::V41RoundScaleScalar)
    );
    let converted = ProgramDefinition::from(program);
    assert_eq!(
        converted.local_round_policy,
        Some(LocalRoundPolicy::V41RoundScaleScalar)
    );
    let definition: ProgramDefinition = serde_json::from_value(selected).unwrap();
    assert_eq!(definition.local_round_policy, converted.local_round_policy);
    let mut restored = serde_json::to_value(converted).unwrap();
    restored
        .as_object_mut()
        .unwrap()
        .remove("local_round_policy");
    assert_eq!(restored, baseline);
}
#[test]
fn source_policy_rejects_unknown_and_wrong_typed_wire_values_after_happy_baseline() {
    let mut value = plain();
    value["local_round_policy"] = "v41_round_scale_scalar".into();
    serde_json::from_value::<ProgramSpec>(value.clone()).unwrap();
    for wrong in [
        serde_json::json!(true),
        serde_json::json!(1),
        serde_json::json!({}),
        serde_json::json!("v41_round_scale_batch"),
    ] {
        value["local_round_policy"] = wrong;
        assert!(serde_json::from_value::<ProgramSpec>(value.clone()).is_err());
        assert!(serde_json::from_value::<ProgramDefinition>(value.clone()).is_err());
    }
}
#[test]
fn named_leaf_preserves_policy_but_composed_or_remapped_source_is_rejected() {
    let mut graph: serde_json::Value =
        serde_json::from_str(include_str!("data/local-round-timetable.json")).unwrap();
    let mut leaf = plain();
    leaf["local_round_policy"] = "v41_round_scale_scalar".into();
    graph["programs"] = serde_json::json!({"leaf":leaf});
    for node in graph["nodes"].as_array_mut().unwrap() {
        if node["kind"] == "compute" {
            node["program"] = "leaf".into();
        }
    }
    serde_json::from_value::<TimetableFile>(graph.clone())
        .unwrap()
        .into_graph()
        .unwrap();
    let inputs = plain()["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| (v.as_str().unwrap().to_owned(), v.clone()))
        .collect::<serde_json::Map<_, _>>();
    graph["programs"]["wrapper"] = serde_json::json!({"inputs":plain()["inputs"],"outputs":["Y"],"calls":[{"program":"leaf","inputs":inputs,"outputs":{"Y":"Y"}}]});
    for node in graph["nodes"].as_array_mut().unwrap() {
        if node["kind"] == "compute" {
            node["program"] = "wrapper".into();
        }
    }
    let error = serde_json::from_value::<TimetableFile>(graph)
        .unwrap()
        .into_graph()
        .unwrap_err();
    assert!(error.0.contains("local round policy"), "{error}");
}
