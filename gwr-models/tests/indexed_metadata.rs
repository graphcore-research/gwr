// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
// Unexecuted API/schema control proposal. Runtime controls remain a later gate.
use gwr_models::processing_element::program::syntax::ProgramSpec;

#[test]
fn indexed_metadata_is_explicit_and_omitted_by_default() {
    let plain = "inputs: []\noutputs: []\nassignments: []\n";
    let baseline: ProgramSpec = yaml_serde::from_str(plain).unwrap();
    assert!(baseline.indexed_metadata_inputs.is_empty());
    let bytes = yaml_serde::to_string(&baseline).unwrap();
    assert!(!bytes.contains("indexed_metadata_inputs"));
    let selected: ProgramSpec = yaml_serde::from_str(
        "inputs: [S]\nmetadata_inputs: [S]\nindexed_metadata_inputs: [S]\noutputs: []\nassignments: []\n"
    ).unwrap();
    assert_eq!(selected.indexed_metadata_inputs, vec!["S"]);
    let quoted: ProgramSpec = yaml_serde::from_str(
        "inputs: ['true']\nmetadata_inputs: ['true']\nindexed_metadata_inputs: ['true']\noutputs: []\nassignments: []\n"
    ).unwrap();
    assert_eq!(quoted.indexed_metadata_inputs, vec!["true"]);

    for malformed in [
        "null",
        "true",
        "[true]",
        "[7]",
        "[null]",
        "[{name: S}]",
        "S",
    ] {
        assert!(
            yaml_serde::from_str::<ProgramSpec>(&format!(
                "inputs: []\noutputs: []\nassignments: []\nindexed_metadata_inputs: {malformed}\n"
            ))
            .is_err(),
            "accepted malformed indexed metadata policy: {malformed}"
        );
    }
}
