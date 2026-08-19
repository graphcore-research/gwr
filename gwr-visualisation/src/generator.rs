// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::path::{Path, PathBuf};
use std::{fs, io};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use gwr_platform::types::PlatformConfig;
use gwr_timetable::timetable_file::TimetableFile;

use crate::analysis::{OverlayInput, summarize};
use crate::payload;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const INDEX_TEMPLATE: &str = include_str!("../assets/index.html");
const STATIC_ASSETS: &[(&str, &str)] = &[
    (
        "gwr_visualisation.js",
        include_str!("../assets/generated/gwr_visualisation.js"),
    ),
    ("bootstrap.js", include_str!("../assets/bootstrap.js")),
    ("style.css", include_str!("../assets/style.css")),
];
const WASM: &[u8] = include_bytes!("../assets/generated/gwr_visualisation_bg.wasm");

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
/// data cannot be serialized, an output aliases an input or another output, or
/// an output file cannot be written.
pub fn write_bundle(inputs: &BundleInputs) -> Result<PathBuf> {
    let timetable = TimetableFile::from_file(&inputs.timetable)
        .map_err(|error| input_error("timetable", &inputs.timetable, error))?;
    timetable
        .validate()
        .map_err(|error| input_error("timetable", &inputs.timetable, error))?;
    let platform = read_platform(inputs.platform.as_deref())?;
    let overlay = read_overlay(inputs.overlay.as_deref())?;

    let mut data = summarize(
        &timetable,
        &inputs.timetable,
        platform.as_ref().zip(inputs.platform.as_deref()),
        overlay.as_ref().zip(inputs.overlay.as_deref()),
    );

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
    reject_path_aliases(
        &inputs.out_dir,
        bundle_files.iter().map(|(name, _)| *name),
        &input_paths,
    )?;

    for (name, contents) in bundle_files {
        fs::write(inputs.out_dir.join(name), contents)?;
    }

    Ok(inputs.out_dir.join("index.html"))
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

fn reject_path_aliases<'a>(
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
            Ok(metadata) if metadata.file_type().is_symlink() => match fs::metadata(output_path) {
                Ok(_) => existing_output_paths.push(output_path),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "Output file '{}' is a dangling symbolic link",
                            output_path.display()
                        ),
                    ));
                }
                Err(error) => return Err(error),
            },
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

fn browser_payload(compressed_data: &[u8], compressed_tensors: &[u8]) -> String {
    format!(
        "window.GWR_VISUALISATION_PAYLOAD={{wasm:\"{}\",data:\"{}\",tensors:\"{}\"}};\n",
        BASE64.encode(WASM),
        BASE64.encode(compressed_data),
        BASE64.encode(compressed_tensors),
    )
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
