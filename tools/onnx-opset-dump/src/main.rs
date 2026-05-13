// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::convert::TryFrom;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use clap::Parser;
use prost::Message;
use serde::Serialize;

pub mod onnx {
    #![allow(missing_docs)]
    #![allow(rustdoc::all)]
    #![allow(clippy::all)]
    #![allow(clippy::pedantic)]
    include!(concat!(env!("OUT_DIR"), "/onnx.rs"));
}

use onnx::attribute_proto::AttributeType;
use onnx::tensor_proto::{DataLocation, DataType};
use onnx::type_proto::Value as TypeValue;
use onnx::{
    AttributeProto, FunctionProto, GraphProto, NodeProto, OperatorProto, OperatorSetIdProto,
    OperatorSetProto, OperatorStatus, SparseTensorProto, StringStringEntryProto, TensorProto,
    TypeProto, ValueInfoProto,
};

type AppResult<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Parser)]
#[command(about = "Render an ONNX OperatorSetProto protobuf as YAML")]
struct Cli {
    /// Input protobuf file containing an onnx.OperatorSetProto
    input: PathBuf,

    /// Optional output file. Defaults to stdout.
    #[arg(long)]
    out: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct OperatorSetDump {
    magic: Option<String>,
    ir_version: Option<i64>,
    ir_version_prerelease: Option<String>,
    ir_build_metadata: Option<String>,
    domain: Option<String>,
    opset_version: Option<i64>,
    doc_string: Option<String>,
    operators: Vec<OperatorDump>,
    functions: Vec<FunctionDump>,
}

#[derive(Debug, Serialize)]
struct OperatorDump {
    op_type: Option<String>,
    since_version: Option<i64>,
    status: Option<String>,
    doc_string: Option<String>,
}

#[derive(Debug, Serialize)]
struct FunctionDump {
    name: Option<String>,
    domain: Option<String>,
    overload: Option<String>,
    doc_string: Option<String>,
    inputs: Vec<String>,
    outputs: Vec<String>,
    attributes: Vec<String>,
    attribute_protos: Vec<AttributeDump>,
    opset_imports: Vec<OperatorSetIdDump>,
    nodes: Vec<NodeDump>,
    value_info: Vec<ValueInfoDump>,
    metadata_props: Vec<MetadataPropDump>,
}

#[derive(Debug, Serialize)]
struct OperatorSetIdDump {
    domain: Option<String>,
    version: Option<i64>,
}

#[derive(Debug, Serialize)]
struct NodeDump {
    name: Option<String>,
    op_type: Option<String>,
    domain: Option<String>,
    overload: Option<String>,
    inputs: Vec<String>,
    outputs: Vec<String>,
    attributes: Vec<AttributeDump>,
    doc_string: Option<String>,
    metadata_props: Vec<MetadataPropDump>,
}

#[derive(Debug, Serialize)]
struct ValueInfoDump {
    name: Option<String>,
    doc_string: Option<String>,
    r#type: Option<TypeDump>,
    metadata_props: Vec<MetadataPropDump>,
}

#[derive(Debug, Serialize)]
struct MetadataPropDump {
    key: Option<String>,
    value: Option<String>,
}

#[derive(Debug, Serialize)]
struct AttributeDump {
    name: Option<String>,
    ref_attr_name: Option<String>,
    doc_string: Option<String>,
    r#type: Option<String>,
    value: AttributeValueDump,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "value")]
#[expect(clippy::large_enum_variant)]
enum AttributeValueDump {
    None,
    Float(f32),
    Int(i64),
    String(String),
    Floats(Vec<f32>),
    Ints(Vec<i64>),
    Strings(Vec<String>),
    Tensor(TensorDump),
    Tensors(Vec<TensorDump>),
    Graph(GraphDump),
    Graphs(Vec<GraphDump>),
    SparseTensor(SparseTensorDump),
    SparseTensors(Vec<SparseTensorDump>),
    Type(TypeDump),
    Types(Vec<TypeDump>),
}

#[derive(Debug, Serialize)]
struct GraphDump {
    name: Option<String>,
    doc_string: Option<String>,
    nodes: Vec<NodeDump>,
    inputs: Vec<ValueInfoDump>,
    outputs: Vec<ValueInfoDump>,
    value_info: Vec<ValueInfoDump>,
    initializers: Vec<TensorDump>,
    sparse_initializers: Vec<SparseTensorDump>,
    metadata_props: Vec<MetadataPropDump>,
}

#[derive(Debug, Serialize)]
struct SparseTensorDump {
    dims: Vec<i64>,
    values: Option<TensorDump>,
    indices: Option<TensorDump>,
}

#[derive(Debug, Serialize)]
struct TensorDump {
    name: Option<String>,
    data_type: Option<String>,
    dims: Vec<i64>,
    doc_string: Option<String>,
    data_location: Option<String>,
    raw_data_bytes: usize,
    float_data_len: usize,
    int32_data_len: usize,
    int64_data_len: usize,
    double_data_len: usize,
    uint64_data_len: usize,
    string_data: Vec<String>,
    external_data: Vec<MetadataPropDump>,
    metadata_props: Vec<MetadataPropDump>,
}

#[derive(Debug, Serialize)]
struct TypeDump {
    kind: String,
    detail: Option<String>,
}

fn main() -> AppResult<()> {
    let cli = Cli::parse();
    let bytes = fs::read(&cli.input)?;
    let opset = OperatorSetProto::decode(bytes.as_slice())
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    let dump = render_opset(&opset);
    let yaml = serde_yaml::to_string(&dump)?;

    match cli.out {
        Some(path) => fs::write(path, yaml)?,
        None => {
            let mut stdout = io::stdout().lock();
            stdout.write_all(yaml.as_bytes())?;
        }
    }

    Ok(())
}

fn render_opset(opset: &OperatorSetProto) -> OperatorSetDump {
    OperatorSetDump {
        magic: clean_string(opset.magic.as_deref()),
        ir_version: opset.ir_version,
        ir_version_prerelease: clean_string(opset.ir_version_prerelease.as_deref()),
        ir_build_metadata: clean_string(opset.ir_build_metadata.as_deref()),
        domain: clean_string(opset.domain.as_deref()),
        opset_version: opset.opset_version,
        doc_string: clean_string(opset.doc_string.as_deref()),
        operators: opset.operator.iter().map(render_operator).collect(),
        functions: opset.functions.iter().map(render_function).collect(),
    }
}

fn render_operator(operator: &OperatorProto) -> OperatorDump {
    OperatorDump {
        op_type: clean_string(operator.op_type.as_deref()),
        since_version: operator.since_version,
        status: operator
            .status
            .and_then(|status| OperatorStatus::try_from(status).ok())
            .map(|status| format!("{status:?}")),
        doc_string: clean_string(operator.doc_string.as_deref()),
    }
}

fn render_function(function: &FunctionProto) -> FunctionDump {
    FunctionDump {
        name: clean_string(function.name.as_deref()),
        domain: clean_string(function.domain.as_deref()),
        overload: clean_string(function.overload.as_deref()),
        doc_string: clean_string(function.doc_string.as_deref()),
        inputs: function.input.clone(),
        outputs: function.output.clone(),
        attributes: function.attribute.clone(),
        attribute_protos: function
            .attribute_proto
            .iter()
            .map(render_attribute)
            .collect(),
        opset_imports: function.opset_import.iter().map(render_opset_id).collect(),
        nodes: function.node.iter().map(render_node).collect(),
        value_info: function.value_info.iter().map(render_value_info).collect(),
        metadata_props: function
            .metadata_props
            .iter()
            .map(render_metadata_prop)
            .collect(),
    }
}

fn render_opset_id(opset_id: &OperatorSetIdProto) -> OperatorSetIdDump {
    OperatorSetIdDump {
        domain: clean_string(opset_id.domain.as_deref()),
        version: opset_id.version,
    }
}

fn render_node(node: &NodeProto) -> NodeDump {
    NodeDump {
        name: clean_string(node.name.as_deref()),
        op_type: clean_string(node.op_type.as_deref()),
        domain: clean_string(node.domain.as_deref()),
        overload: clean_string(node.overload.as_deref()),
        inputs: node.input.clone(),
        outputs: node.output.clone(),
        attributes: node.attribute.iter().map(render_attribute).collect(),
        doc_string: clean_string(node.doc_string.as_deref()),
        metadata_props: node
            .metadata_props
            .iter()
            .map(render_metadata_prop)
            .collect(),
    }
}

fn render_value_info(value_info: &ValueInfoProto) -> ValueInfoDump {
    ValueInfoDump {
        name: clean_string(value_info.name.as_deref()),
        doc_string: clean_string(value_info.doc_string.as_deref()),
        r#type: value_info.r#type.as_ref().map(render_type),
        metadata_props: value_info
            .metadata_props
            .iter()
            .map(render_metadata_prop)
            .collect(),
    }
}

fn render_metadata_prop(prop: &StringStringEntryProto) -> MetadataPropDump {
    MetadataPropDump {
        key: clean_string(prop.key.as_deref()),
        value: clean_string(prop.value.as_deref()),
    }
}

fn render_attribute(attribute: &AttributeProto) -> AttributeDump {
    AttributeDump {
        name: clean_string(attribute.name.as_deref()),
        ref_attr_name: clean_string(attribute.ref_attr_name.as_deref()),
        doc_string: clean_string(attribute.doc_string.as_deref()),
        r#type: attribute
            .r#type
            .and_then(|kind| AttributeType::try_from(kind).ok())
            .map(|kind| format!("{kind:?}")),
        value: render_attribute_value(attribute),
    }
}

fn render_attribute_value(attribute: &AttributeProto) -> AttributeValueDump {
    if let Some(value) = attribute.f {
        AttributeValueDump::Float(value)
    } else if let Some(value) = attribute.i {
        AttributeValueDump::Int(value)
    } else if let Some(value) = &attribute.s {
        AttributeValueDump::String(lossy_bytes(value))
    } else if let Some(value) = &attribute.t {
        AttributeValueDump::Tensor(render_tensor(value))
    } else if let Some(value) = &attribute.g {
        AttributeValueDump::Graph(render_graph(value))
    } else if let Some(value) = &attribute.sparse_tensor {
        AttributeValueDump::SparseTensor(render_sparse_tensor(value))
    } else if let Some(value) = &attribute.tp {
        AttributeValueDump::Type(render_type(value))
    } else if !attribute.floats.is_empty() {
        AttributeValueDump::Floats(attribute.floats.clone())
    } else if !attribute.ints.is_empty() {
        AttributeValueDump::Ints(attribute.ints.clone())
    } else if !attribute.strings.is_empty() {
        AttributeValueDump::Strings(
            attribute
                .strings
                .iter()
                .map(|bytes| lossy_bytes(bytes))
                .collect(),
        )
    } else if !attribute.tensors.is_empty() {
        AttributeValueDump::Tensors(attribute.tensors.iter().map(render_tensor).collect())
    } else if !attribute.graphs.is_empty() {
        AttributeValueDump::Graphs(attribute.graphs.iter().map(render_graph).collect())
    } else if !attribute.sparse_tensors.is_empty() {
        AttributeValueDump::SparseTensors(
            attribute
                .sparse_tensors
                .iter()
                .map(render_sparse_tensor)
                .collect(),
        )
    } else if !attribute.type_protos.is_empty() {
        AttributeValueDump::Types(attribute.type_protos.iter().map(render_type).collect())
    } else {
        AttributeValueDump::None
    }
}

fn render_graph(graph: &GraphProto) -> GraphDump {
    GraphDump {
        name: clean_string(graph.name.as_deref()),
        doc_string: clean_string(graph.doc_string.as_deref()),
        nodes: graph.node.iter().map(render_node).collect(),
        inputs: graph.input.iter().map(render_value_info).collect(),
        outputs: graph.output.iter().map(render_value_info).collect(),
        value_info: graph.value_info.iter().map(render_value_info).collect(),
        initializers: graph.initializer.iter().map(render_tensor).collect(),
        sparse_initializers: graph
            .sparse_initializer
            .iter()
            .map(render_sparse_tensor)
            .collect(),
        metadata_props: graph
            .metadata_props
            .iter()
            .map(render_metadata_prop)
            .collect(),
    }
}

fn render_sparse_tensor(tensor: &SparseTensorProto) -> SparseTensorDump {
    SparseTensorDump {
        dims: tensor.dims.clone(),
        values: tensor.values.as_ref().map(render_tensor),
        indices: tensor.indices.as_ref().map(render_tensor),
    }
}

fn render_tensor(tensor: &TensorProto) -> TensorDump {
    TensorDump {
        name: clean_string(tensor.name.as_deref()),
        data_type: tensor
            .data_type
            .and_then(|dtype| DataType::try_from(dtype).ok())
            .map(|dtype| format!("{dtype:?}")),
        dims: tensor.dims.clone(),
        doc_string: clean_string(tensor.doc_string.as_deref()),
        data_location: tensor
            .data_location
            .and_then(|location| DataLocation::try_from(location).ok())
            .map(|location| format!("{location:?}")),
        raw_data_bytes: tensor.raw_data.as_ref().map_or(0, Vec::len),
        float_data_len: tensor.float_data.len(),
        int32_data_len: tensor.int32_data.len(),
        int64_data_len: tensor.int64_data.len(),
        double_data_len: tensor.double_data.len(),
        uint64_data_len: tensor.uint64_data.len(),
        string_data: tensor
            .string_data
            .iter()
            .map(|bytes| lossy_bytes(bytes))
            .collect(),
        external_data: tensor
            .external_data
            .iter()
            .map(render_metadata_prop)
            .collect(),
        metadata_props: tensor
            .metadata_props
            .iter()
            .map(render_metadata_prop)
            .collect(),
    }
}

fn render_type(tp: &TypeProto) -> TypeDump {
    let detail = match tp.value.as_ref() {
        Some(TypeValue::TensorType(tensor)) => Some(format!(
            "tensor(elem_type={}, shape={})",
            tensor
                .elem_type
                .and_then(|dtype| DataType::try_from(dtype).ok())
                .map_or_else(|| "UNKNOWN".to_string(), |dtype| format!("{dtype:?}")),
            render_shape(tensor.shape.as_ref())
        )),
        Some(TypeValue::SequenceType(_)) => Some("sequence".to_string()),
        Some(TypeValue::MapType(_)) => Some("map".to_string()),
        Some(TypeValue::OptionalType(_)) => Some("optional".to_string()),
        Some(TypeValue::SparseTensorType(tensor)) => Some(format!(
            "sparse_tensor(elem_type={}, shape={})",
            tensor
                .elem_type
                .and_then(|dtype| DataType::try_from(dtype).ok())
                .map_or_else(|| "UNKNOWN".to_string(), |dtype| format!("{dtype:?}")),
            render_shape(tensor.shape.as_ref())
        )),
        None => None,
    };

    let kind = match tp.value.as_ref() {
        Some(TypeValue::TensorType(_)) => "tensor",
        Some(TypeValue::SequenceType(_)) => "sequence",
        Some(TypeValue::MapType(_)) => "map",
        Some(TypeValue::OptionalType(_)) => "optional",
        Some(TypeValue::SparseTensorType(_)) => "sparse_tensor",
        None => "unknown",
    }
    .to_string();

    TypeDump { kind, detail }
}

fn clean_string(value: Option<&str>) -> Option<String> {
    value.and_then(|text| {
        let trimmed = text.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn lossy_bytes(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

fn render_shape(shape: Option<&onnx::TensorShapeProto>) -> String {
    let Some(shape) = shape else {
        return "?".to_string();
    };

    let dims = shape
        .dim
        .iter()
        .map(|dim| match dim.value.as_ref() {
            Some(onnx::tensor_shape_proto::dimension::Value::DimValue(value)) => value.to_string(),
            Some(onnx::tensor_shape_proto::dimension::Value::DimParam(value)) => value.clone(),
            None => "?".to_string(),
        })
        .collect::<Vec<_>>();

    format!("[{}]", dims.join(", "))
}
