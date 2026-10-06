// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use std::path::PathBuf;
use std::process::exit;
use std::{fs, io};

use clap::Parser;
use regex::Regex;

const GENERAL_PATTERN: &str = r"Copyright \(c\) 202[0-9] Graphcore Ltd\. All rights reserved\.";
const LICENSE_PATTERN: &str = r"Copyright \(c\) 202[0-9] Graphcore Ltd\.\n";

const LICENSE_FILENAME: &str = "LICENSE";

const FAILURE_STATUS: i32 = 1;

/// Command-line arguments.
#[derive(Parser)]
#[command(about = "Check files contains the correct copyright notice")]
struct Cli {
    /// Paths to the files to be checked
    #[clap(required = true)]
    files: Vec<String>,
}

fn main() -> io::Result<()> {
    let args = Cli::parse();
    let gen_re = Regex::new(GENERAL_PATTERN).expect("`GENERAL_PATTERN` should be a valid regex");
    let lic_re = Regex::new(LICENSE_PATTERN).expect("`LICENSE_PATTERN` should be a valid regex");

    let mut re: &Regex;
    let mut failures = Vec::new();

    for filename in args.files {
        let content = fs::read_to_string(&filename)?;
        let abs_path = PathBuf::from(filename).canonicalize()?;

        if abs_path.file_name().unwrap() == LICENSE_FILENAME {
            re = &lic_re;
        } else {
            re = &gen_re;
        }

        if !re.is_match(&content) {
            failures.push(format!(
                "No valid copyright notice found in {}",
                abs_path.display()
            ));
        }
    }

    if !failures.is_empty() {
        let err_msg = failures
            .iter()
            .map(String::from)
            .reduce(|acc, e| format!("{acc}\n{e}"))
            .unwrap();
        println!("{err_msg}");
        exit(FAILURE_STATUS);
    }

    Ok(())
}
