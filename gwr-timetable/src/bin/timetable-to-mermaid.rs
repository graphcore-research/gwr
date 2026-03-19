// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use clap::Parser;
use gwr_timetable::mermaid::render_mermaid_from_parts;
use gwr_timetable::timetable_file::TimetableFile;

#[derive(Debug, Clone, Parser)]
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
    let args = Cli::parse();
    let input = args.timetable;
    let output = args.mermaid;

    let yaml = fs::read_to_string(&input)?;
    let timetable: TimetableFile = serde_yaml::from_str(&yaml)?;
    let mermaid = render_mermaid_from_parts(&timetable.nodes, &timetable.edges, &HashMap::new());

    fs::write(output, mermaid)?;

    Ok(())
}
