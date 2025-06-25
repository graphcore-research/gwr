// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! A command-line documentation pre-processing utility
//!
//! This utility is designed to work on the output of `cargo expand`
//! which will have done any pre-processing.
//!
//! For example:
//! ```bash
//! > pushd steam-engine
//! > STEAM_DOC_BUILDER=true cargo expand --lib > expanded.rs
//! > popd
//! > adoc-builder -i expanded.rs -o doc.adoc
//! ```
//!
//! For latest usage run:
//! ```bash
//! > adoc-builder --help
//! ```
use std::error::Error;
use std::fs::File;

use clap::Parser;
use steam_doc_builder::doc_parser::DocParser;

/// Command-line arguments.
#[derive(Parser)]
#[command(about = "Document pre-processor")]
struct Cli {
    /// Path of the file to pre-process.
    #[arg(short, long)]
    input_file: String,

    /// Path of the output file to write.
    #[arg(short, long)]
    output_file: String,

    /// Path of the top-level block containing the TOC which defines the
    /// structure of the document to create.
    #[arg(short, long)]
    top: String,

    /// Dump all blocks found in the document.
    #[arg(short, long)]
    dump_all: bool,

    /// Emit verbose logging.
    #[arg(short, long)]
    verbose: bool,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Cli::parse();
    let mut parser = DocParser::new(args.verbose);
    let top = parser.parse_doc(&args.input_file);

    let mut out_file = File::create(&args.output_file)?;

    if args.dump_all {
        top.borrow().dump();
    }

    parser.write_adoc(&mut out_file, &args.top);

    Ok(())
}
