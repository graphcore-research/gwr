// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::path::PathBuf;

use clap::Parser;
use gwr_engine::engine::Engine;
use gwr_platform::Platform;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Parser)]
#[command(about = "Load and validate a platform configuration file")]
struct Args {
    /// Platform YAML file to validate.
    #[arg(long, default_value = "platform.yaml")]
    platform: PathBuf,

    /// Print the constructed platform after validation.
    #[arg(long, default_value_t = false)]
    print_platform: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let platform = Platform::from_file(&engine, &clock, &args.platform)?;

    println!(
        "Validated '{}' with {} PEs, {} caches, {} memories, and {} fabrics.",
        args.platform.display(),
        platform.num_pes(),
        platform.num_caches(),
        platform.num_memories(),
        platform.num_fabrics()
    );

    if args.print_platform {
        println!("{platform}");
    }

    Ok(())
}
