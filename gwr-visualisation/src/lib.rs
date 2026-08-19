// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Generate a self-contained browser report from GWR timetable data.

#![warn(missing_docs)]

mod analysis;

use std::path::{Path, PathBuf};
use std::{fs, io};

use analysis::{OverlayInput, summarize};
use gwr_platform::types::PlatformConfig;
use gwr_timetable::timetable_file::TimetableFile;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const INDEX_TEMPLATE: &str = include_str!("../assets/index.html");
const STATIC_ASSETS: &[(&str, &str)] = &[
    (
        "view-model.js",
        include_str!("../benchmarks/legacy/assets/view-model.js"),
    ),
    (
        "core.js",
        include_str!("../benchmarks/legacy/assets/core.js"),
    ),
    (
        "filters.js",
        include_str!("../benchmarks/legacy/assets/filters.js"),
    ),
    (
        "pe-grid.js",
        include_str!("../benchmarks/legacy/assets/pe-grid.js"),
    ),
    (
        "timetable.js",
        include_str!("../benchmarks/legacy/assets/timetable.js"),
    ),
    (
        "tensors.js",
        include_str!("../benchmarks/legacy/assets/tensors.js"),
    ),
    (
        "memory.js",
        include_str!("../benchmarks/legacy/assets/memory.js"),
    ),
    (
        "relationships.js",
        include_str!("../benchmarks/legacy/assets/relationships.js"),
    ),
    (
        "workspace.js",
        include_str!("../benchmarks/legacy/assets/workspace.js"),
    ),
    ("app.js", include_str!("../benchmarks/legacy/assets/app.js")),
    (
        "benchmark-hooks.js",
        include_str!("../benchmarks/legacy/assets/benchmark-hooks.js"),
    ),
    ("style.css", include_str!("../assets/style.css")),
];

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

    let data = summarize(
        &timetable,
        &inputs.timetable,
        platform.as_ref().zip(inputs.platform.as_deref()),
        overlay.as_ref().zip(inputs.overlay.as_deref()),
    );

    fs::create_dir_all(&inputs.out_dir)?;

    let data_json = serde_json::to_string_pretty(&data)?;
    let compact_data = serde_json::to_string(&data)?;
    let data_js = format!("window.GWR_VISUALISATION_DATA={compact_data};\n");
    let index_html = INDEX_TEMPLATE.replace("{{DATA_SCRIPT}}", "data.js");

    fs::write(inputs.out_dir.join("index.html"), index_html)?;
    fs::write(inputs.out_dir.join("data.json"), data_json)?;
    fs::write(inputs.out_dir.join("data.js"), data_js)?;
    for (name, contents) in STATIC_ASSETS {
        fs::write(inputs.out_dir.join(name), contents)?;
    }

    Ok(inputs.out_dir.join("index.html"))
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
