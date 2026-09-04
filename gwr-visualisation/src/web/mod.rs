// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

mod address;
#[cfg(all(feature = "web", target_arch = "wasm32"))]
mod app;
mod format;
mod logic;
mod relationship_geometry;
mod relationships;
#[cfg(all(feature = "web", target_arch = "wasm32"))]
mod render;
mod state;
#[cfg(all(feature = "web", target_arch = "wasm32"))]
mod workspace;
mod workspace_model;

#[cfg(all(feature = "web", target_arch = "wasm32"))]
use js_sys::Uint8Array;
#[cfg(all(feature = "web", target_arch = "wasm32"))]
use logic::AppModel;
#[cfg(all(feature = "web", target_arch = "wasm32"))]
use wasm_bindgen::JsValue;
#[cfg(all(feature = "web", target_arch = "wasm32"))]
use wasm_bindgen::prelude::wasm_bindgen;

#[cfg(all(feature = "web", target_arch = "wasm32"))]
use crate::model::{ReportData, TensorSummary};

#[cfg(all(feature = "web", target_arch = "wasm32"))]
#[wasm_bindgen]
/// Start the browser report from serialized report data.
pub fn run(serialized: Uint8Array) -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    let bytes = copy_bytes(&serialized);
    let data = crate::payload::decode_json::<ReportData>(&bytes)
        .map_err(|error| JsValue::from_str(&error))?;
    let model = AppModel::new(data);
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| JsValue::from_str("Unable to access the report document"))?;
    app::start(model, document)
}

#[cfg(all(feature = "web", target_arch = "wasm32"))]
#[wasm_bindgen]
/// Attach serialized tensor details after the initial summary is rendered.
pub fn load_tensors(serialized: Uint8Array) -> Result<(), JsValue> {
    let bytes = copy_bytes(&serialized);
    let tensors = crate::payload::decode_json::<Vec<TensorSummary>>(&bytes)
        .map_err(|error| JsValue::from_str(&error))?;
    app::attach_tensors(tensors)
}

#[cfg(all(feature = "web", target_arch = "wasm32"))]
fn copy_bytes(serialized: &Uint8Array) -> Vec<u8> {
    let mut bytes = vec![0; serialized.length() as usize];
    serialized.copy_to(&mut bytes);
    bytes
}

#[cfg(all(feature = "web", target_arch = "wasm32"))]
#[wasm_bindgen]
/// Run a deterministic report kernel and return a checksum for benchmark
/// validation.
pub fn benchmark_kernel(name: &str, iterations: u32) -> Result<f64, JsValue> {
    app::benchmark_kernel(name, iterations)
}
