// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::io;
use std::path::{Path, PathBuf};

use clap::Parser;
use gwr_visualisation::{BundleInputs, write_bundle};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Parser)]
#[command(about = "Generate a static web visualisation bundle for a GWR timetable")]
struct Cli {
    /// Timetable YAML file to visualise
    #[arg(long)]
    timetable: PathBuf,

    /// Directory to write the static report bundle into
    #[arg(long)]
    out: PathBuf,

    /// Optional platform YAML file to overlay known PE layout and config
    #[arg(long)]
    platform: Option<PathBuf>,

    /// Optional metric overlay JSON file
    #[arg(long)]
    overlay: Option<PathBuf>,

    /// Open the generated index.html in the system browser
    #[arg(long)]
    open: bool,
}

fn open_in_browser(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg(path);
        command
    };

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.arg("/C").arg("start").arg("").arg(path);
        command
    };

    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(path);
        command
    };

    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("browser command exited with status {status}")).into())
    }
}

fn main() -> Result<()> {
    let args = Cli::parse();
    let index = write_bundle(&BundleInputs {
        timetable: args.timetable,
        platform: args.platform,
        overlay: args.overlay,
        out_dir: args.out,
    })?;

    println!("Wrote visualisation bundle to {}", index.display());

    if args.open {
        open_in_browser(&index)?;
    }

    Ok(())
}
