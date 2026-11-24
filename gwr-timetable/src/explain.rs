// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fmt::Write as _;

use clap::ValueEnum;
use gwr_engine::types::SimError;
use gwr_models::processing_element::dispatch::Dispatch;
use gwr_models::processing_element::program::validate::{RegionBindings, validate_program};
use gwr_models::processing_element::task::Task;
use gwr_models::processing_element::tensors::{HasShape, TensorView};

use crate::Timetable;

impl Timetable {
    /// Describe program-task bindings, tiling, workspace, and logical traffic
    /// along with effective PE configuration, without starting simulation.
    ///
    /// Output size is proportional to graph metadata. Computing the planned
    /// traffic may traverse tiles. Call [`Self::preflight`] first to check task
    /// admission, including capabilities, SRAM capacity, and memory mappings.
    ///
    /// # Errors
    ///
    /// Returns an error if task metadata cannot be resolved or a workspace or
    /// traffic estimate fails.
    pub fn preparation_description(&self) -> Result<String, SimError> {
        let mut out = String::from(
            "Preparation summary (logical planned bytes; physical traffic is not observed):\n",
        );
        let mut pe_names = self.platform.pe_names();
        pe_names.sort();
        for name in pe_names {
            let pe = self.platform.pe(&name)?;
            writeln!(out, "PE {name}: {}", pe.configuration_description()).unwrap();
        }
        for (index, node) in self.graph.nodes().iter().enumerate() {
            if node.program().is_none() {
                continue;
            }
            let Task::ProgramTask { config } = self.task_by_id(index)? else {
                continue;
            };
            let pe_name = node.pe().expect("program task placement");
            let pe = self.platform.pe(pe_name)?;
            let program = &config.program;
            let region = validate_program(
                program,
                &RegionBindings {
                    inputs: config.inputs.clone(),
                    outputs: config.outputs.clone(),
                },
            )?;
            let declared = config.tiling.as_ref().map(|tiling| &tiling.sizes);

            writeln!(out, "Node {} on PE {pe_name}:", node.id()).unwrap();
            for (names, bindings, direction, arrow) in [
                (&program.inputs, &config.inputs, "input", "<-"),
                (&program.outputs, &config.outputs, "output", "->"),
            ] {
                for (name, view) in names.iter().zip(bindings) {
                    write!(out, "  {direction} {name} {arrow} ").unwrap();
                    write_binding(&mut out, view);
                }
            }
            writeln!(
                out,
                "  extents: {}",
                axis_sizes(
                    region
                        .extents
                        .iter()
                        .map(|(axis, size)| (axis.as_str(), *size))
                )
            )
            .unwrap();
            writeln!(
                out,
                "  declared tile sizes: {}",
                axis_sizes(
                    declared
                        .into_iter()
                        .flatten()
                        .map(|(axis, size)| (axis.as_str(), *size))
                )
            )
            .unwrap();
            writeln!(
                out,
                "  effective tile sizes: {} (maximum; tail tiles may be smaller)",
                axis_sizes(region.extents.iter().map(|(axis, &extent)| {
                    let step = declared.and_then(|sizes| sizes.get(axis)).copied();
                    (axis.as_str(), step.unwrap_or(extent).min(extent))
                }))
            )
            .unwrap();
            let peak = pe.program_workspace_bytes(&config)?;
            writeln!(out, "  peak SRAM: {peak} bytes").unwrap();
            let (read, written) = config.planned_traffic_bytes()?;
            writeln!(
                out,
                "  logical boundary traffic: read={read} bytes, write={written} bytes"
            )
            .unwrap();
        }
        Ok(out)
    }
}

fn write_binding(out: &mut String, view: &TensorView) {
    let tensor = view.tensor();
    writeln!(
        out,
        "{} dtype={} shape=[{}] offsets=[{}] tensor base={:#x}",
        tensor.id().unwrap_or("<unnamed>"),
        view.dtype()
            .to_possible_value()
            .expect("datatype name")
            .get_name(),
        dimensions(view.shape().dims()),
        dimensions(view.offsets()),
        tensor.addr(),
    )
    .unwrap();
}

fn dimensions(values: &[usize]) -> String {
    values
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn axis_sizes<'a>(sizes: impl Iterator<Item = (&'a str, usize)>) -> String {
    let sizes = sizes
        .map(|(axis, size)| format!("{axis}={size}"))
        .collect::<Vec<_>>();
    if sizes.is_empty() {
        "none".to_owned()
    } else {
        sizes.join(", ")
    }
}
