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
/// data cannot be serialized, or an output file cannot be written.
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

    fs::create_dir_all(&inputs.out_dir)?;

    let data_json = serde_json::to_string_pretty(&data)?;
    let tensors = std::mem::take(&mut data.tensors);
    let compressed_data = payload::encode(&data)?;
    let compressed_tensors = payload::encode(&tensors)?;
    let payload_js = browser_payload(&compressed_data, &compressed_tensors);

    fs::write(inputs.out_dir.join("index.html"), INDEX_TEMPLATE)?;
    fs::write(inputs.out_dir.join("data.json"), data_json)?;
    fs::write(inputs.out_dir.join("payload.js"), payload_js)?;
    for (name, contents) in STATIC_ASSETS {
        fs::write(inputs.out_dir.join(name), contents)?;
    }

    Ok(inputs.out_dir.join("index.html"))
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
