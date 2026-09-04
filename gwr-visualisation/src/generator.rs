// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::path::{Path, PathBuf};
use std::{fs, io};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use gwr_platform::types::PlatformConfig;
use gwr_timetable::timetable_file::TimetableFile;

use crate::analysis::{OverlayInput, build_report};
use crate::payload;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const INDEX_TEMPLATE: &str = include_str!("../assets/index.html");
const STATIC_ASSETS: &[(&str, &str)] = &[
    ("bootstrap.js", include_str!("../assets/bootstrap.js")),
    ("view-model.js", include_str!("../assets/view-model.js")),
    ("core.js", include_str!("../assets/core.js")),
    ("filters.js", include_str!("../assets/filters.js")),
    ("pe-grid.js", include_str!("../assets/pe-grid.js")),
    ("timetable.js", include_str!("../assets/timetable.js")),
    ("tensors.js", include_str!("../assets/tensors.js")),
    ("memory.js", include_str!("../assets/memory.js")),
    (
        "relationships.js",
        include_str!("../assets/relationships.js"),
    ),
    ("workspace.js", include_str!("../assets/workspace.js")),
    ("app.js", include_str!("../assets/app.js")),
    ("style.css", include_str!("../assets/style.css")),
];
const RETIRED_OUTPUTS: &[&str] = &["data.js"];

/// Input files and destination for a generated report bundle.
#[derive(Debug)]
pub struct BundleInputs {
    /// Timetable YAML to analyse.
    pub timetable: PathBuf,
    /// Optional platform YAML used for PE and memory topology.
    pub platform: Option<PathBuf>,
    /// Optional JSON containing per-PE metrics.
    pub overlay: Option<PathBuf>,
    /// Directory into which the static report files are written.
    pub out_dir: PathBuf,
}

/// Generate a static report and return the path to its `index.html`.
///
/// # Errors
///
/// Returns an error when an input cannot be read, parsed, or validated, report
/// data cannot be serialized, an output is unsafe to replace, or an output
/// file cannot be written.
pub fn write_bundle(inputs: &BundleInputs) -> Result<PathBuf> {
    let graph = TimetableFile::from_file(&inputs.timetable)
        .map_err(|error| input_error("timetable", &inputs.timetable, error))?;
    let graph = graph
        .into_graph()
        .map_err(|error| input_error("timetable", &inputs.timetable, error))?;
    let platform = read_platform(inputs.platform.as_deref())?;
    let overlay = read_overlay(inputs.overlay.as_deref())?;

    let mut data = build_report(
        &graph,
        &inputs.timetable,
        platform.as_ref().zip(inputs.platform.as_deref()),
        overlay.as_ref().zip(inputs.overlay.as_deref()),
    )?;

    let data_json = serde_json::to_string_pretty(&data)?;
    let tensors = std::mem::take(&mut data.tensors);
    let compressed_data = payload::encode(&data)?;
    let compressed_tensors = payload::encode(&tensors)?;
    let payload_js = browser_payload(&compressed_data, &compressed_tensors);
    let bundle_files: Vec<_> = [
        ("index.html", INDEX_TEMPLATE),
        ("data.json", data_json.as_str()),
        ("payload.js", payload_js.as_str()),
    ]
    .into_iter()
    .chain(STATIC_ASSETS.iter().copied())
    .collect();

    let input_paths = input_paths(inputs);
    fs::create_dir_all(&inputs.out_dir)?;
    check_output_files(
        &inputs.out_dir,
        bundle_files.iter().map(|(name, _)| *name),
        &input_paths,
    )?;

    let staging_dir = tempfile::Builder::new()
        .prefix(".gwr-visualisation-")
        .tempdir_in(&inputs.out_dir)?;
    for (name, contents) in &bundle_files {
        fs::write(staging_dir.path().join(name), contents)?;
    }
    remove_retired_outputs(&inputs.out_dir)?;
    for (name, _) in bundle_files {
        replace_output(&staging_dir.path().join(name), &inputs.out_dir.join(name))?;
    }

    Ok(inputs.out_dir.join("index.html"))
}

fn browser_payload(compressed_data: &[u8], compressed_tensors: &[u8]) -> String {
    format!(
        "window.GWR_VISUALISATION_PAYLOAD={{data:\"{}\",tensors:\"{}\"}};\n",
        BASE64.encode(compressed_data),
        BASE64.encode(compressed_tensors),
    )
}

fn input_paths(inputs: &BundleInputs) -> Vec<&Path> {
    [
        Some(inputs.timetable.as_path()),
        inputs.platform.as_deref(),
        inputs.overlay.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn check_output_files<'a>(
    out_dir: &Path,
    output_names: impl IntoIterator<Item = &'a str>,
    input_paths: &[&Path],
) -> io::Result<()> {
    let output_paths = output_names
        .into_iter()
        .map(|name| out_dir.join(name))
        .collect::<Vec<_>>();
    let mut existing_output_paths = Vec::new();
    for output_path in &output_paths {
        match fs::symlink_metadata(output_path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Output file '{}' is a symbolic link", output_path.display()),
                ));
            }
            Ok(metadata) if !metadata.is_file() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "Output path '{}' is not a regular file",
                        output_path.display()
                    ),
                ));
            }
            Ok(_) => existing_output_paths.push(output_path),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }

    for output_path in &existing_output_paths {
        for input_path in input_paths {
            if same_file::is_same_file(input_path, output_path)? {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "Output file '{}' aliases input file '{}'",
                        output_path.display(),
                        input_path.display()
                    ),
                ));
            }
        }
    }

    for (output_idx, output_path) in existing_output_paths.iter().enumerate() {
        for other_output_path in &existing_output_paths[..output_idx] {
            if same_file::is_same_file(other_output_path, output_path)? {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "Output file '{}' aliases output file '{}'",
                        output_path.display(),
                        other_output_path.display()
                    ),
                ));
            }
        }
    }

    Ok(())
}

fn replace_output(staged: &Path, destination: &Path) -> io::Result<()> {
    match fs::symlink_metadata(destination) {
        Ok(metadata) if metadata.is_dir() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "Output path '{}' is not a regular file",
                    destination.display()
                ),
            ));
        }
        Ok(_) => fs::remove_file(destination)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::rename(staged, destination)
}

fn remove_retired_outputs(out_dir: &Path) -> io::Result<()> {
    for name in RETIRED_OUTPUTS {
        let path = out_dir.join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Retired output path '{}' is a directory", path.display()),
                ));
            }
            Ok(_) => fs::remove_file(path)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
fn read_platform(path: Option<&Path>) -> Result<Option<PlatformConfig>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let contents =
        fs::read_to_string(path).map_err(|error| input_error("platform", path, error))?;
    let platform: PlatformConfig =
        serde_yaml::from_str(&contents).map_err(|error| input_error("platform", path, error))?;
    platform
        .validate()
        .map_err(|error| input_error("platform", path, error))?;
    Ok(Some(platform))
}

fn read_overlay(path: Option<&Path>) -> Result<Option<OverlayInput>> {
    Ok(path
        .map(|path| {
            let contents =
                fs::read_to_string(path).map_err(|error| input_error("overlay", path, error))?;
            serde_json::from_str(&contents).map_err(|error| input_error("overlay", path, error))
        })
        .transpose()?)
}

fn input_error(kind: &str, path: &Path, error: impl std::fmt::Display) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("Unable to load {kind} file '{}': {error}", path.display()),
    )
}
