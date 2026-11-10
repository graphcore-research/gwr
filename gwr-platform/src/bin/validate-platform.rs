// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::path::PathBuf;

use gwr_config::multi_source_config;
use gwr_engine::engine::Engine;
use gwr_platform::Platform;

type AppResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[multi_source_config]
#[derive(Debug)]
#[command(about = "Load and validate a platform configuration file")]
struct Args {
    /// Platform YAML file to validate.
    #[arg(long, default_value_t = PathBuf::from("platform.yaml"))]
    platform: Option<PathBuf>,

    /// Print the constructed platform after validation.
    #[arg(long, default_value_t = false)]
    print_platform: bool,

    /// Print derived fabric port maps after validation.
    #[arg(long, default_value_t = false)]
    print_fabric_port_maps: bool,
}

fn main() -> AppResult<()> {
    let args = Args::parse_all_sources();
    let platform_path = args.platform.unwrap();

    let mut engine = Engine::default();
    let clock = engine.default_clock();
    let platform = Platform::from_file(&engine, &clock, &platform_path)?;

    println!(
        "Validated '{}' with {} PEs, {} caches, {} memories, and {} fabrics.",
        platform_path.display(),
        platform.num_pes(),
        platform.num_caches(),
        platform.num_memories(),
        platform.num_fabrics()
    );

    if args.print_platform {
        println!("{platform}");
    }

    if args.print_fabric_port_maps {
        print!("{}", platform.fabric_port_map_description());
    }

    Ok(())
}
