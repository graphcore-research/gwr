// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use gwr_tools::{
    CoverageReport, RenderOptions, combined_total_coverage_is_non_negative,
    render_markdown_with_options,
};

#[derive(Debug, Parser)]
#[command(about = "Compare two llvm-cov summary JSON files")]
struct Args {
    /// Baseline llvm-cov summary.json file.
    before: PathBuf,

    /// Comparison llvm-cov summary.json file.
    after: PathBuf,

    /// Include unchanged files in the file-level table.
    #[arg(long, value_enum, default_value_t = Files::Changed)]
    files: Files,

    /// Folder prefix to remove from filenames in the baseline report.
    #[arg(long)]
    before_prefix: Option<String>,

    /// Folder prefix to remove from filenames in the comparison report.
    #[arg(long)]
    after_prefix: Option<String>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Files {
    Changed,
    All,
}

fn main() -> Result<ExitCode, Box<dyn std::error::Error>> {
    let args = Args::parse();
    let before_path = args.before.display().to_string();
    let after_path = args.after.display().to_string();
    let before = CoverageReport::from_path(&args.before)?;
    let after = CoverageReport::from_path(&args.after)?;
    let show_all_files = match args.files {
        Files::Changed => false,
        Files::All => true,
    };

    print!(
        "{}",
        render_markdown_with_options(
            &before,
            &after,
            &RenderOptions {
                show_all_files,
                before_path: Some(before_path),
                after_path: Some(after_path),
                before_prefix: args.before_prefix,
                after_prefix: args.after_prefix,
            },
        )
    );

    if combined_total_coverage_is_non_negative(&before, &after) {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}
