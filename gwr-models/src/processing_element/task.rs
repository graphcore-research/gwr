// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_engine::sim_error;
use gwr_engine::types::SimError;
use rand::RngExt;
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};

use crate::processing_element::operators::{
    ExpansionDirection, HasShape, Operator, OperatorAdd, OperatorCustom, OperatorGemm,
    OperatorMaxPool, Shape, Tensor, TensorPartition, TensorView, create_maxpool_op,
    maybe_add_indices_output, partition_tensors,
};
use crate::processing_element::{ComputeCapabilities, MachineOpCounts};

#[derive(Debug, Clone)]
pub struct ComputeTaskConfig {
    /// Only needed as a debug aid
    pub id: String,
    pub op: ComputeOp,
    pub inputs: Vec<Option<TensorView>>,
    pub outputs: Vec<Option<TensorView>>,
}

impl ComputeTaskConfig {
    #[must_use]
    pub fn activity_name(&self) -> &str {
        match &self.op {
            ComputeOp::Custom(operator) => operator.name.as_deref().unwrap_or(&self.id),
            _ => &self.id,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComputeOp {
    Add,
    Gemm,
    MaxPool(OperatorMaxPool),
    Custom(OperatorCustom),
}

impl Serialize for ComputeOp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Add => serializer.serialize_str("add"),
            Self::Gemm => serializer.serialize_str("gemm"),
            Self::MaxPool(operator) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("maxpool", operator)?;
                map.end()
            }
            Self::Custom(operator) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("custom", operator)?;
                map.end()
            }
        }
    }
}

impl ComputeOp {
    #[must_use]
    pub fn trace_name(&self) -> &str {
        match self {
            ComputeOp::Add => "add",
            ComputeOp::Gemm => "gemm",
            ComputeOp::MaxPool(_) => "maxpool",
            ComputeOp::Custom(operator) => operator.name.as_deref().unwrap_or("custom"),
        }
    }

    pub fn compute_delay_ticks(
        &self,
        compute_capabilities: &Rc<ComputeCapabilities>,
        input_views: &[Option<TensorView>],
        output_views: &[Option<TensorView>],
    ) -> Result<usize, SimError> {
        self.operator()
            .compute_delay_ticks(compute_capabilities, input_views, output_views)
    }

    pub fn validate(
        &self,
        input_views: &[Option<TensorView>],
        output_views: &[Option<TensorView>],
    ) -> Result<(), SimError> {
        self.operator().validate(input_views, output_views)
    }

    pub fn compute_machine_ops(
        &self,
        input_views: &[Option<TensorView>],
        output_views: &[Option<TensorView>],
    ) -> Result<MachineOpCounts, SimError> {
        self.operator()
            .compute_machine_ops(input_views, output_views)
    }

    pub fn create_partitions(
        &self,
        input_views: &[Option<TensorView>],
        output_views: &[Option<TensorView>],
        num_partitions: usize,
    ) -> Result<Vec<TensorPartition>, SimError> {
        self.operator()
            .partition_views(input_views, output_views, num_partitions)?
            .collect()
    }

    pub(crate) fn create_partitions_for_sram(
        &self,
        input_views: &[Option<TensorView>],
        output_views: &[Option<TensorView>],
        sram_bytes: usize,
    ) -> Result<Vec<TensorPartition>, SimError> {
        let operator = self.operator();
        let max_partition_count = operator.max_partition_count(input_views, output_views)?;
        let mut candidate_count = 1;
        let Some(mut oversized_working_set) = first_oversized_working_set(
            operator,
            input_views,
            output_views,
            candidate_count,
            sram_bytes,
        )?
        else {
            return self.create_partitions(input_views, output_views, candidate_count);
        };
        if sram_bytes == 0 {
            return sim_error!("Compute task requires memory but the PE has no SRAM");
        }

        let mut failing_count = candidate_count;
        loop {
            if candidate_count == max_partition_count {
                return sim_error!(
                    "{} cannot fit in {sram_bytes} bytes of SRAM: a partition at the maximum useful partition count requires {oversized_working_set} bytes",
                    self.trace_name(),
                );
            }

            candidate_count = candidate_count.saturating_mul(2).min(max_partition_count);
            let Some(working_set_bytes) = first_oversized_working_set(
                operator,
                input_views,
                output_views,
                candidate_count,
                sram_bytes,
            )?
            else {
                break;
            };
            oversized_working_set = working_set_bytes;
            failing_count = candidate_count;
        }

        let mut fitting_count = candidate_count;
        while fitting_count - failing_count > 1 {
            let middle = failing_count + (fitting_count - failing_count) / 2;
            if first_oversized_working_set(operator, input_views, output_views, middle, sram_bytes)?
                .is_none()
            {
                fitting_count = middle;
            } else {
                failing_count = middle;
            }
        }
        self.create_partitions(input_views, output_views, fitting_count)
    }

    pub fn create_tensor_partitions(
        &self,
        inputs: &[Option<Tensor>],
        outputs: &[Option<Tensor>],
        num_partitions: usize,
    ) -> Result<Vec<TensorPartition>, SimError> {
        partition_tensors(self.operator(), inputs, outputs, num_partitions)
    }

    pub fn create_inputs(
        &self,
        outputs: &[Option<Tensor>],
        expand_ratio: f64,
        rng: &mut impl RngExt,
    ) -> Result<Vec<Option<Tensor>>, SimError> {
        match self {
            Self::Add => OperatorAdd::create_inputs(outputs, expand_ratio, rng),
            Self::Gemm => OperatorGemm::create_inputs(outputs, expand_ratio, rng),
            Self::MaxPool(operator) => operator.create_inputs(outputs, expand_ratio, rng),
            Self::Custom(_) => Err(SimError(
                "Custom operators cannot generate tensors".to_string(),
            )),
        }
    }

    pub fn create_outputs(
        &self,
        inputs: &[Option<Tensor>],
        expand_ratio: f64,
        rng: &mut impl RngExt,
    ) -> Result<Vec<Option<Tensor>>, SimError> {
        match self {
            Self::Add => OperatorAdd::create_outputs(inputs, expand_ratio, rng),
            Self::Gemm => OperatorGemm::create_outputs(inputs, expand_ratio, rng),
            Self::MaxPool(operator) => operator.create_outputs(inputs, expand_ratio, rng),
            Self::Custom(_) => Err(SimError(
                "Custom operators cannot generate tensors".to_string(),
            )),
        }
    }

    pub fn configured_for_tensor<T: HasShape>(
        &self,
        tensor: &T,
        direction: ExpansionDirection,
        expand_ratio: f64,
    ) -> Result<Self, SimError> {
        match self {
            Self::Add => Ok(Self::Add),
            Self::Gemm => Ok(Self::Gemm),
            Self::MaxPool(_) => Ok(Self::MaxPool(create_maxpool_op(
                tensor,
                direction,
                expand_ratio,
            )?)),
            Self::Custom(_) => Err(SimError("Custom operators cannot be generated".to_string())),
        }
    }

    pub fn add_optional_inputs(
        &self,
        inputs: &mut Vec<Option<Tensor>>,
        expand_ratio: f64,
        rng: &mut impl RngExt,
    ) -> Result<bool, SimError> {
        match self {
            Self::Gemm => super::operators::maybe_add_input_c(inputs, expand_ratio, rng),
            _ => Ok(false),
        }
    }

    pub fn add_optional_outputs(
        &self,
        outputs: &mut Vec<Option<Tensor>>,
        expand_ratio: f64,
        rng: &mut impl RngExt,
    ) -> Result<bool, SimError> {
        match self {
            Self::MaxPool(_) => maybe_add_indices_output(outputs, expand_ratio, rng),
            _ => Ok(false),
        }
    }

    pub fn gemm_rhs_shape<T: HasShape>(input: &T) -> Result<Shape, SimError> {
        super::operators::gemm_rhs_shape(input)
    }

    fn operator(&self) -> &dyn Operator {
        static ADD: OperatorAdd = OperatorAdd;
        static GEMM: OperatorGemm = OperatorGemm;

        match self {
            Self::Add => &ADD,
            Self::Gemm => &GEMM,
            Self::MaxPool(operator) => operator,
            Self::Custom(operator) => operator,
        }
    }
}

fn first_oversized_working_set(
    operator: &dyn Operator,
    input_views: &[Option<TensorView>],
    output_views: &[Option<TensorView>],
    num_partitions: usize,
    sram_bytes: usize,
) -> Result<Option<usize>, SimError> {
    for partition in operator.partition_views(input_views, output_views, num_partitions)? {
        let working_set_bytes = partition?.working_set_bytes()?;
        if working_set_bytes > sram_bytes {
            return Ok(Some(working_set_bytes));
        }
    }
    Ok(None)
}

#[derive(Debug, Clone, Copy)]
pub enum SyncRegion {
    Local,
    Global,
}

#[derive(Debug, Clone)]
pub enum Task {
    ComputeTask { config: ComputeTaskConfig },
    SyncTask { region: SyncRegion },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processing_element::operators::dtype::DataType;

    fn largest_working_set(partitions: &[TensorPartition]) -> Result<usize, SimError> {
        partitions.iter().try_fold(0usize, |largest, partition| {
            Ok(largest.max(partition.working_set_bytes()?))
        })
    }

    fn view(dims: &[usize], dtype: DataType) -> Option<TensorView> {
        Some(TensorView::new_full(Tensor::new(dims, &dtype, 0).unwrap()))
    }

    fn assert_working_sets_do_not_increase(
        op: &ComputeOp,
        inputs: &[Option<TensorView>],
        outputs: &[Option<TensorView>],
    ) {
        let max_count = op.operator().max_partition_count(inputs, outputs).unwrap();
        let mut previous = usize::MAX;
        for requested in 1..=max_count {
            let partitions = op.create_partitions(inputs, outputs, requested).unwrap();
            let largest = largest_working_set(&partitions).unwrap();
            assert!(
                largest <= previous,
                "{} working set increased from {previous} to {largest} at {requested} partitions",
                op.trace_name()
            );
            previous = largest;
        }
    }

    #[test]
    fn refines_add_partitions_until_they_fit() {
        let inputs = vec![view(&[3], DataType::Int8), view(&[3], DataType::Int8)];
        let outputs = vec![view(&[3], DataType::Int8)];

        let partitions = ComputeOp::Add
            .create_partitions_for_sram(&inputs, &outputs, 5)
            .unwrap();

        assert_eq!(partitions.len(), 3);
        assert_eq!(largest_working_set(&partitions).unwrap(), 3);
    }

    #[test]
    fn refines_gemm_partitions_until_they_fit() {
        let inputs = vec![
            view(&[10, 10], DataType::Bf16),
            view(&[10, 10], DataType::Bf16),
        ];
        let outputs = vec![view(&[10, 10], DataType::Bf16)];

        let partitions = ComputeOp::Gemm
            .create_partitions_for_sram(&inputs, &outputs, 250)
            .unwrap();

        assert_eq!(partitions.len(), 10);
        assert!(largest_working_set(&partitions).unwrap() <= 250);
    }

    #[test]
    fn refines_maxpool_partitions_for_overlapping_input_windows() {
        let op = ComputeOp::MaxPool(OperatorMaxPool::new(&[3]));
        let inputs = vec![view(&[1, 1, 10], DataType::Int8)];
        let outputs = vec![view(&[1, 1, 8], DataType::Int8)];

        let partitions = op.create_partitions_for_sram(&inputs, &outputs, 9).unwrap();

        assert_eq!(partitions.len(), 3);
        assert!(largest_working_set(&partitions).unwrap() <= 9);
    }

    #[test]
    fn uses_one_maxpool_partition_when_trailing_input_is_unused() {
        let mut operator = OperatorMaxPool::new(&[3]);
        operator.strides = Some(vec![3]);
        let inputs = vec![view(&[1, 1, 10], DataType::Int8)];
        let outputs = vec![view(&[1, 1, 3], DataType::Int8)];

        let partitions = ComputeOp::MaxPool(operator)
            .create_partitions_for_sram(&inputs, &outputs, 12)
            .unwrap();

        assert_eq!(partitions.len(), 1);
        assert_eq!(largest_working_set(&partitions).unwrap(), 12);
    }

    #[test]
    fn partition_working_set_overflow_returns_error() {
        let tensor = Tensor::new(&[usize::MAX / 2], &DataType::Int8, 0).unwrap();
        let view = Some(TensorView::new_full(tensor));
        let inputs = vec![view.clone(), view.clone()];
        let outputs = vec![view];

        let error = ComputeOp::Add
            .create_partitions_for_sram(&inputs, &outputs, usize::MAX)
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Tensor partition byte count overflows")
        );
    }

    #[test]
    fn rejects_an_irreducible_custom_working_set() {
        let op = ComputeOp::Custom(OperatorCustom {
            name: None,
            machine_ops: MachineOpCounts::default(),
        });
        let inputs = vec![view(&[6], DataType::Int8)];

        let error = op.create_partitions_for_sram(&inputs, &[], 5).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("maximum useful partition count requires 6 bytes")
        );
    }

    #[test]
    fn rejects_an_irreducible_gemm_without_materializing_every_partition() {
        let inputs = vec![
            view(&[10_000, 10_000], DataType::Bf16),
            view(&[10_000, 10_000], DataType::Bf16),
        ];
        let outputs = vec![view(&[10_000, 10_000], DataType::Bf16)];

        let error = ComputeOp::Gemm
            .create_partitions_for_sram(&inputs, &outputs, 32 * 1024)
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("maximum useful partition count requires 40002 bytes")
        );
    }

    #[test]
    fn operator_working_sets_do_not_increase_with_finer_partitions() {
        assert_working_sets_do_not_increase(
            &ComputeOp::Add,
            &[view(&[3, 4], DataType::Int8), view(&[3, 4], DataType::Int8)],
            &[view(&[3, 4], DataType::Int8)],
        );
        assert_working_sets_do_not_increase(
            &ComputeOp::Gemm,
            &[view(&[4, 4], DataType::Bf16), view(&[4, 4], DataType::Bf16)],
            &[view(&[4, 4], DataType::Bf16)],
        );
        assert_working_sets_do_not_increase(
            &ComputeOp::MaxPool(OperatorMaxPool::new(&[3])),
            &[view(&[1, 2, 8], DataType::Int8)],
            &[view(&[1, 2, 6], DataType::Int8)],
        );
    }
}
