// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
#[test]
fn source_round_policy_carrier_is_required_after_valid_ordinary_baseline() {
    let document: serde_json::Value =
        serde_json::from_str(include_str!("data/local-round-timetable.json")).unwrap();
    let mut value = document["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["kind"] == "compute")
        .unwrap()["program"]
        .clone();
    serde_json::from_value::<gwr_models::processing_element::program::syntax::ProgramSpec>(
        value.clone(),
    )
    .unwrap();
    serde_json::from_value::<gwr_timetable::timetable_file::ProgramDefinition>(value.clone())
        .unwrap();
    value["local_round_policy"] = "v41_round_scale_scalar".into();
    serde_json::from_value::<gwr_models::processing_element::program::syntax::ProgramSpec>(
        value.clone(),
    )
    .expect("intended old closed carrier lacks exact source policy");
    serde_json::from_value::<gwr_timetable::timetable_file::ProgramDefinition>(value)
        .expect("source policy required in reusable carrier");
}
