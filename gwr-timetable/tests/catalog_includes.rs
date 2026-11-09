// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fs;

use gwr_timetable::timetable_file::TimetableFile;

#[test]
fn imported_program_can_compose_another_imported_program() {
    let dir = std::env::temp_dir().join(format!(
        "gwr-timetable-catalog-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("base.yaml"), "programs: { copy: { inputs: [A], outputs: [Y], assignments: [{ output: Y, axes: [i], expr: { read: { tensor: A, indices: [i] } } }] } }\n").unwrap();
    fs::write(dir.join("composed.yaml"), "includes: [base.yaml]\nprograms: { twice: { inputs: [A], outputs: [Y], calls: [{ program: copy, inputs: { A: A }, outputs: { Y: Y } }] } }\n").unwrap();
    fs::write(
        dir.join("graph.yaml"),
        "includes: [composed.yaml]\nnodes: []\nedges: []\n",
    )
    .unwrap();
    let file = TimetableFile::from_file(&dir.join("graph.yaml")).unwrap();
    assert!(file.programs.contains_key("copy"));
    assert_eq!(file.programs["twice"].calls[0].program, "copy");
    assert!(
        TimetableFile::from_string("includes: [composed.yaml]\nnodes: []\nedges: []")
            .unwrap_err()
            .to_string()
            .contains("from_file")
    );
}
