// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Synthetic tensor shapes and program templates for the random timetable
//! generator.

use gwr_engine::sim_error;
use gwr_engine::types::SimError;
use gwr_models::processing_element::program::TensorSpec;
use gwr_models::processing_element::program::syntax::ProgramSpec;
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_models::processing_element::tensors::{HasShape, Shape, Tensor, TensorView};
use rand::RngExt;

use super::programs::{build_add_program, build_gemm_program, build_reduce_max_program};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExpansionDirection {
    Backward,
    Forward,
}

/// A random workload choice. Every generated node contains a tensor program.
#[derive(Clone, Debug)]
pub enum ProgramTemplate {
    Add,
    Gemm,
    ReduceMax,
}

impl ProgramTemplate {
    pub fn build_program(
        &self,
        inputs: &[Option<TensorView>],
        outputs: &[Option<TensorView>],
        matrix_accumulator: DataType,
    ) -> Result<ProgramSpec, SimError> {
        let input_specs = inputs
            .iter()
            .flatten()
            .map(tensor_spec_from_view)
            .collect::<Vec<_>>();
        let output_specs = outputs
            .iter()
            .flatten()
            .map(tensor_spec_from_view)
            .collect::<Vec<_>>();
        let output = output_specs
            .first()
            .ok_or_else(|| SimError("generated program has no output".into()))?;
        match self {
            Self::Add => build_add_program(&input_specs, output),
            Self::Gemm => build_gemm_program(&input_specs, output, matrix_accumulator),
            Self::ReduceMax => {
                let input = input_specs
                    .first()
                    .ok_or_else(|| SimError("max reduction has no input".into()))?;
                build_reduce_max_program(input, output_specs.len() == 2)
            }
        }
    }

    pub fn create_inputs(
        &self,
        outputs: &[Option<Tensor>],
        ratio: f64,
        optional_input_ratio: f64,
        rng: &mut impl RngExt,
    ) -> Result<Vec<Option<Tensor>>, SimError> {
        let output = require_first_tensor(outputs, "output")?;
        let shape = output.shape().dims();
        match self {
            Self::Add => Ok(vec![
                create_tensor(shape, output.dtype())?,
                create_tensor(shape, output.dtype())?,
            ]),
            Self::Gemm => {
                if shape.len() < 2 {
                    return sim_error!("matrix output requires rank at least two");
                }
                let mut lhs = shape.to_vec();
                let mut rhs = shape.to_vec();
                let k = ((shape[shape.len() - 2].max(shape[shape.len() - 1]) as f64) * ratio)
                    .round()
                    .max(1.0) as usize;
                lhs[shape.len() - 1] = k;
                rhs[shape.len() - 2] = k;
                let mut inputs = vec![
                    create_tensor(&lhs, output.dtype())?,
                    create_tensor(&rhs, output.dtype())?,
                ];
                self.add_optional_inputs(&mut inputs, optional_input_ratio, rng)?;
                Ok(inputs)
            }
            Self::ReduceMax => sim_error!("max reduction expands only forward"),
        }
    }

    pub fn create_outputs(
        &self,
        inputs: &[Option<Tensor>],
        ratio: f64,
        rng: &mut impl RngExt,
    ) -> Result<Vec<Option<Tensor>>, SimError> {
        let input = require_first_tensor(inputs, "input")?;
        let shape = input.shape().dims();
        match self {
            Self::Add => Ok(vec![create_tensor(shape, input.dtype())?]),
            Self::Gemm => {
                let rhs = inputs.get(1).and_then(Option::as_ref).ok_or_else(|| {
                    SimError("matrix product is missing its right operand".into())
                })?;
                if shape.len() < 2 || rhs.shape().num_dims() != shape.len() {
                    return sim_error!("matrix operand ranks differ");
                }
                let mut output = shape.to_vec();
                output[shape.len() - 1] = rhs.shape().dims()[shape.len() - 1];
                Ok(vec![create_tensor(&output, input.dtype())?])
            }
            Self::ReduceMax => {
                if shape.len() < 2 {
                    return sim_error!("max reduction requires rank at least two");
                }
                let output_shape = shape[..shape.len() - 1].to_vec();
                let mut outputs = vec![create_tensor(&output_shape, input.dtype())?];
                if sample_optional(ratio, rng) {
                    outputs.push(create_tensor(&output_shape, &DataType::Int64)?);
                }
                Ok(outputs)
            }
        }
    }

    pub fn add_optional_inputs(
        &self,
        inputs: &mut Vec<Option<Tensor>>,
        ratio: f64,
        rng: &mut impl RngExt,
    ) -> Result<bool, SimError> {
        if !matches!(self, Self::Gemm) || inputs.len() >= 3 || !sample_optional(ratio, rng) {
            return Ok(false);
        }
        let lhs = require_first_tensor(inputs, "matrix left operand")?;
        let rhs = inputs
            .get(1)
            .and_then(Option::as_ref)
            .ok_or_else(|| SimError("matrix product is missing its right operand".into()))?;
        let n = rhs.shape().dims()[rhs.shape().num_dims() - 1];
        inputs.push(create_tensor(&[n], lhs.dtype())?);
        Ok(true)
    }

    pub fn add_optional_outputs(
        &self,
        outputs: &mut Vec<Option<Tensor>>,
        ratio: f64,
        rng: &mut impl RngExt,
    ) -> Result<bool, SimError> {
        if !matches!(self, Self::ReduceMax) || outputs.len() >= 2 || !sample_optional(ratio, rng) {
            return Ok(false);
        }
        let output = require_first_tensor(outputs, "max reduction output")?;
        outputs.push(create_tensor(output.shape().dims(), &DataType::Int64)?);
        Ok(true)
    }

    pub fn gemm_rhs_shape<T: HasShape>(input: &T) -> Result<Shape, SimError> {
        if input.num_dims() < 2 {
            return sim_error!("matrix left operand requires rank at least two");
        }
        let mut shape = input.shape().dims().to_vec();
        let len = shape.len();
        shape[len - 2] = input.shape().dims()[len - 1];
        shape[len - 1] = input.shape().dims()[len - 2];
        Shape::new(&shape)
    }
}

fn require_first_tensor<'a, T>(items: &'a [Option<T>], label: &str) -> Result<&'a T, SimError> {
    items
        .first()
        .and_then(Option::as_ref)
        .ok_or_else(|| SimError(format!("generated {label} is missing")))
}

fn create_tensor(shape: &[usize], dtype: &DataType) -> Result<Option<Tensor>, SimError> {
    Ok(Some(Tensor::new(shape, dtype, 0)?))
}

fn tensor_spec_from_view(view: &TensorView) -> TensorSpec {
    TensorSpec {
        shape: view.shape().dims().to_vec(),
        dtype: *view.dtype(),
    }
}

fn sample_optional(ratio: f64, rng: &mut impl RngExt) -> bool {
    ratio >= 1.0 || (ratio > 0.0 && rng.random_bool(ratio))
}

#[cfg(test)]
mod tests {
    use gwr_models::processing_element::tensors::dtype::DataType;
    use gwr_models::processing_element::tensors::{HasShape, Tensor};
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    use super::ProgramTemplate;

    const OUTPUT_SHAPE: [usize; 2] = [2, 3];
    const PRIMARY_ADDRESS: u64 = 0x1000;
    const INDEX_ADDRESS: u64 = 0x2000;
    const SEED: u64 = 0;

    fn primary_output() -> Option<Tensor> {
        Some(
            Tensor::new(&OUTPUT_SHAPE, &DataType::Fp16, PRIMARY_ADDRESS)
                .unwrap()
                .with_id("primary"),
        )
    }

    fn assert_outputs_unchanged(actual: &[Option<Tensor>], expected: &[Option<Tensor>]) {
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(expected) {
            match (actual, expected) {
                (Some(actual), Some(expected)) => {
                    assert_eq!(actual.shape().dims(), expected.shape().dims());
                    assert_eq!(actual.dtype(), expected.dtype());
                    assert_eq!(actual.addr(), expected.addr());
                    assert_eq!(actual.id(), expected.id());
                    assert_eq!(actual.num_bytes(), expected.num_bytes());
                }
                (None, None) => {}
                _ => panic!("optional output presence changed"),
            }
        }
    }

    #[test]
    fn reduce_max_adds_matching_int64_indices_without_changing_primary_output() {
        let mut outputs = vec![primary_output()];
        let primary = outputs.clone();
        let mut rng = StdRng::seed_from_u64(SEED);
        assert!(
            ProgramTemplate::ReduceMax
                .add_optional_outputs(&mut outputs, 1.0, &mut rng)
                .unwrap()
        );
        assert_eq!(outputs.len(), 2);
        assert_outputs_unchanged(&outputs[..1], &primary);
        let indices = outputs[1].as_ref().unwrap();
        assert_eq!(indices.shape().dims(), OUTPUT_SHAPE);
        assert_eq!(indices.dtype(), &DataType::Int64);
    }

    #[test]
    fn other_templates_leave_optional_outputs_unchanged() {
        for template in [ProgramTemplate::Add, ProgramTemplate::Gemm] {
            let mut outputs = vec![primary_output()];
            let before = outputs.clone();
            let mut rng = StdRng::seed_from_u64(SEED);
            assert!(
                !template
                    .add_optional_outputs(&mut outputs, 1.0, &mut rng)
                    .unwrap()
            );
            assert_outputs_unchanged(&outputs, &before);
        }
    }

    #[test]
    fn reduce_max_keeps_existing_indices_output() {
        let indices = Tensor::new(&OUTPUT_SHAPE, &DataType::Int64, INDEX_ADDRESS)
            .unwrap()
            .with_id("indices");
        let mut outputs = vec![primary_output(), Some(indices)];
        let before = outputs.clone();
        let mut rng = StdRng::seed_from_u64(SEED);
        assert!(
            !ProgramTemplate::ReduceMax
                .add_optional_outputs(&mut outputs, 1.0, &mut rng)
                .unwrap()
        );
        assert_outputs_unchanged(&outputs, &before);
    }

    #[test]
    fn zero_optional_output_ratio_keeps_values_only_reduction() {
        let mut outputs = vec![primary_output()];
        let before = outputs.clone();
        let mut rng = StdRng::seed_from_u64(SEED);
        assert!(
            !ProgramTemplate::ReduceMax
                .add_optional_outputs(&mut outputs, 0.0, &mut rng)
                .unwrap()
        );
        assert_outputs_unchanged(&outputs, &before);
    }

    #[test]
    fn reduce_max_requires_primary_output_before_adding_indices() {
        for mut outputs in [vec![], vec![None]] {
            let before = outputs.clone();
            let mut rng = StdRng::seed_from_u64(SEED);
            let error = ProgramTemplate::ReduceMax
                .add_optional_outputs(&mut outputs, 1.0, &mut rng)
                .unwrap_err();
            assert!(
                error
                    .0
                    .contains("generated max reduction output is missing"),
                "{error}"
            );
            assert_outputs_unchanged(&outputs, &before);
        }
    }
}
