// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::fmt::Write as _;

use clap::ValueEnum;
use gwr_models::processing_element::tensors::{HasShape, TensorView};

use crate::Timetable;

impl Timetable {
    /// Describe retained preparation facts without running or revisiting tiles.
    ///
    /// Output is deterministic and proportional to graph metadata,
    /// independently of the number of tile invocations. Logical boundary bytes
    /// precede LSU packetization; they are not physical traffic measurements.
    #[must_use]
    pub fn preparation_description(&self) -> String {
        let mut out = String::from(
            "Preparation summary (logical planned bytes; physical traffic is not observed):\n",
        );
        let mut pe_names = self.platform.pe_names();
        pe_names.sort();
        for name in pe_names {
            let pe = self.platform.pe(&name).expect("retained PE name");
            writeln!(out, "PE {name}: {}", pe.configuration_description()).unwrap();
        }
        for (index, node) in self.graph.nodes().iter().enumerate() {
            let Some(task) = self.prepared_tasks.get(&index) else {
                continue;
            };
            let program = task.program();
            writeln!(out, "Node {} on PE {}:", node.id(), node.pe().unwrap()).unwrap();
            let views = self
                .graph
                .compute_views(index)
                .expect("prepared task views");
            for (names, bindings, direction, arrow) in [
                (program.input_names(), views.inputs(), "input", "<-"),
                (program.output_names(), views.outputs(), "output", "->"),
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
                    program
                        .extents()
                        .iter()
                        .map(|(axis, size)| (axis.as_str(), *size))
                )
            )
            .unwrap();
            writeln!(
                out,
                "  declared tile sizes: {}",
                axis_sizes(
                    program
                        .declared_tile_sizes()
                        .iter()
                        .map(|(axis, size)| (axis.as_str(), *size))
                )
            )
            .unwrap();
            writeln!(
                out,
                "  effective tile sizes: {} (maximum; tail tiles may be smaller)",
                axis_sizes(program.effective_tile_sizes())
            )
            .unwrap();
            writeln!(out, "  peak SRAM: {} bytes", task.peak_sram_bytes()).unwrap();
            let (read, written) = task.planned_traffic_bytes();
            writeln!(
                out,
                "  logical boundary traffic: read={read} bytes, write={written} bytes"
            )
            .unwrap();
        }
        out
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
