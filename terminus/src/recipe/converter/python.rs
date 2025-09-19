// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! Write out a recipe as a Python script.
//!
//!  TODO: Write it out so that it can run interactive commands (e.g.
//! history/fc).
use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::Path;

use crate::recipe::Recipe;

const HEADER: &str = "# Auto-generated Python script\n";

fn python_main(description: &str) -> String {
    format!(
        "import argparse
import os
import subprocess

def main():
    parser = argparse.ArgumentParser(description=\"{description}\")
"
    )
}

fn python_arg(name: &str, help: &str) -> String {
    format!(
        "
    parser.add_argument(
        \"{name}\",
        type=str,
        help=\"{help}\",
    )
"
    )
}

fn python_arg_default(name: &str, help: &str, default: &str) -> String {
    format!(
        "    parser.add_argument(
        \"--{name}\",
        type=str,
        help=\"{help}\",
        default=\"{default}\",
    )
"
    )
}

fn python_export_arg(name: &str) -> String {
    format!("    os.environ[\"{name}\"] = args.{name}\n")
}

fn python_run_command(command: &str) -> String {
    format!("    subprocess.run(\'{command}\', shell=True)\n")
}

fn python_tail() -> String {
    "
if __name__ == \"__main__\":
    main()
"
    .to_string()
}

/// Write out the Recipe as an equivalent Python script
pub fn convert_to(recipe: &Recipe, out_path: &Path) -> io::Result<()> {
    let file = fs::File::create(out_path)?;
    let mut bin_writer = Box::new(BufWriter::new(file));

    let mut lines = Vec::new();
    lines.push(HEADER.to_string());
    lines.push(python_main(recipe.description.as_str()));
    for arg in &recipe.arguments {
        match &arg.default {
            Some(default_value) => lines.push(python_arg_default(
                arg.name.as_str(),
                arg.comment.as_str(),
                default_value.as_str(),
            )),
            None => lines.push(python_arg(arg.name.as_str(), arg.comment.as_str())),
        }
    }
    lines.push("    args = parser.parse_args()\n\n".to_string());
    lines.push("\n    # Export variables to environment\n".to_string());
    for arg in &recipe.arguments {
        lines.push(python_export_arg(arg.name.as_str()));
    }

    lines.push("\n    # Run commands\n".to_string());
    for ingedient in &recipe.ingredients {
        lines.push(python_run_command(ingedient.command.as_str()));
    }
    lines.push(python_tail());

    for line in lines {
        bin_writer.write_all(line.as_bytes())?;
    }
    Ok(())
}
