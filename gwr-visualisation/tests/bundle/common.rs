// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::process::Command;

pub(super) const SMALL_TIMETABLE: &str = "../gwr-timetable/examples/small.yaml";

pub(super) fn generator_command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_gwr-visualisation"))
}

pub(super) struct GeneratedReport {
    pub(super) temp: tempfile::TempDir,
    pub(super) index_html: String,
    pub(super) data: serde_json::Value,
}

impl GeneratedReport {
    pub(super) fn generate() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let output = generator_command()
            .arg("--timetable")
            .arg(SMALL_TIMETABLE)
            .arg("--platform")
            .arg("../gwr-platform/examples/platform_4x4.yaml")
            .arg("--out")
            .arg(temp.path())
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "gwr-visualisation failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let index_html = std::fs::read_to_string(temp.path().join("index.html")).unwrap();
        let data_json = std::fs::read_to_string(temp.path().join("data.json")).unwrap();
        let data = serde_json::from_str(&data_json).unwrap();
        Self {
            temp,
            index_html,
            data,
        }
    }

    pub(super) fn asset(&self, name: &str) -> String {
        std::fs::read_to_string(self.temp.path().join(name))
            .unwrap_or_else(|error| panic!("unable to read generated {name}: {error}"))
    }
}
