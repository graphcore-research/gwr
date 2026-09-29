// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use gwr_config::multi_source_config;
use gwr_timetable::mermaid::render_mermaid;
use gwr_timetable::timetable_file::TimetableFile;

#[multi_source_config]
#[derive(Debug, Clone)]
#[command(about = "Convert a GWR Timetable to a Mermaid diagram")]
struct Cli {
    #[arg(long)]
    timetable: PathBuf,

    #[arg(long)]
    platform: PathBuf,

    #[arg(long)]
    mermaid: PathBuf,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Cli {
        timetable: input,
        platform,
        mermaid: output,
    } = Cli::parse_all_sources();
    drop(platform);

    let yaml = fs::read_to_string(&input)?;
    let timetable: TimetableFile = serde_yaml::from_str(&yaml)?;
    let graph = timetable.into_graph()?;
    let mermaid = render_mermaid(&graph, &HashMap::new());

    fs::write(output, mermaid)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Cli, CliPartial};

    #[test]
    #[should_panic(expected = "missing required configuration value `timetable`")]
    fn timetable_path_is_required() {
        Cli::partial_to_config(CliPartial::default());
    }

    #[test]
    #[should_panic(expected = "missing required configuration value `platform`")]
    fn platform_path_is_required() {
        Cli::partial_to_config(CliPartial {
            timetable: Some(PathBuf::from("timetable.yaml")),
            ..CliPartial::default()
        });
    }

    #[test]
    #[should_panic(expected = "missing required configuration value `mermaid`")]
    fn mermaid_path_is_required() {
        Cli::partial_to_config(CliPartial {
            timetable: Some(PathBuf::from("timetable.yaml")),
            platform: Some(PathBuf::from("platform.yaml")),
            ..CliPartial::default()
        });
    }
}
