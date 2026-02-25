#!/usr/bin/env python3

# Copyright (c) 2026 Graphcore Ltd. All rights reserved.

# Read a model from an ONNX proto file and a gwr-platform YAML file and output a
# gwr-timetable YAML file that will simulate the model.
#
# Notes on robustness for large/dynamic ONNX graphs:
# - If tensor metadata is incomplete (for example, missing shape in value_info),
#   conversion falls back to a placeholder single-element shape [1].
# - If tensor dtype is missing, conversion defaults to FLOAT/fp32.
# - Empty optional node inputs/outputs are skipped.
# These fallbacks allow timetable generation to proceed for models where full
# static shape inference is unavailable.

import argparse
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

try:
    import onnx  # type: ignore
    from onnx import TensorProto  # type: ignore
except Exception as e:  # pragma: no cover
    raise SystemExit(
        f"Failed to import 'onnx'. Install it with: pip install onnx\n{e}"
    ) from e

try:
    import yaml  # type: ignore
except Exception as e:  # pragma: no cover
    raise SystemExit(
        f"Failed to import 'pyyaml'. Install it with: pip install pyyaml\n{e}"
    ) from e


# Default SRAM size per PE (from builder.rs DEFAULT_SRAM_BYTES)
DEFAULT_SRAM_BYTES: int = 1024 * 1024  # 1MB

# Mapping from ONNX data types to byte sizes
_DTYPE_SIZES: Dict[int, int] = {
    TensorProto.FLOAT: 4,
    TensorProto.UINT8: 1,
    TensorProto.INT8: 1,
    TensorProto.UINT16: 2,
    TensorProto.INT16: 2,
    TensorProto.INT32: 4,
    TensorProto.INT64: 8,
    TensorProto.BOOL: 1,
    TensorProto.FLOAT16: 2,
    TensorProto.DOUBLE: 8,
    TensorProto.UINT32: 4,
    TensorProto.UINT64: 8,
    TensorProto.COMPLEX64: 8,
    TensorProto.COMPLEX128: 16,
    TensorProto.BFLOAT16: 2,
}


def _get_tensor_num_elements(shape: List[Optional[int]]) -> int:
    """Calculate the number of elements in a tensor given its shape."""
    num_elements = 1
    for dim in shape:
        if dim is None or dim <= 0:
            # Unknown or dynamic dimension - use 1 as placeholder
            num_elements *= 1
        else:
            num_elements *= dim

    return num_elements


def _get_tensor_size_bytes(shape: List[Optional[int]], dtype: int) -> int:
    """Calculate the size in bytes of a tensor given its shape and data type."""
    # Calculate number of elements
    num_elements = _get_tensor_num_elements(shape)

    # Get size per element
    elem_size_bytes = _DTYPE_SIZES.get(dtype, 4)  # Default to 32-bit if unknown

    return num_elements * elem_size_bytes


def _shape_from_type_proto(type_proto: Any) -> Optional[List[Optional[int]]]:
    """Extract shape from ONNX type proto."""
    if not type_proto or not type_proto.HasField("tensor_type"):
        return None
    tensor_type = type_proto.tensor_type
    if not tensor_type.HasField("shape"):
        return None
    out: List[Optional[int]] = []
    for dim in tensor_type.shape.dim:
        if dim.HasField("dim_value"):
            out.append(int(dim.dim_value))
        elif dim.HasField("dim_param"):
            # Currently not needed by gwr-timetable
            out.append(None)
        else:
            out.append(None)
    return out


def _dtype_from_type_proto(type_proto: Any) -> Optional[int]:
    """Extract dtype from ONNX type proto."""
    if not type_proto or not type_proto.HasField("tensor_type"):
        return None
    tensor_type = type_proto.tensor_type
    if not tensor_type.HasField("elem_type"):
        return None
    return int(tensor_type.elem_type)


def _collect_tensors(
    model: Any,
) -> Dict[str, Tuple[Optional[List[Optional[int]]], int, int]]:
    """
    Collect all tensors from the ONNX graph.
    Returns a dict mapping tensor name to (shape, dtype, size_bytes).
    """
    graph = model.graph
    tensors: Dict[str, Tuple[Optional[List[Optional[int]]], int, int]] = {}

    initializer_map: Dict[str, Any] = {init.name: init for init in graph.initializer}

    # Get tensors from value_info, graph inputs, and graph outputs
    for vi in list(graph.value_info) + list(graph.input) + list(graph.output):
        shape = _shape_from_type_proto(vi.type)
        dtype = _dtype_from_type_proto(vi.type)

        # If the tensor has no declared shape/dtype but is a constant, use the
        # initializer's metadata.
        init = initializer_map.get(vi.name)
        if shape is None and init is not None and hasattr(init, "dims"):
            shape = list(init.dims)
        if dtype is None and init is not None and hasattr(init, "data_type"):
            dtype = int(init.data_type)

        if dtype is None:
            dtype = TensorProto.FLOAT

        if shape is None:
            # Fall back to a single-element placeholder for dynamic/unknown shapes.
            # This enables conversions of models with incomplete shape info.
            shape = [1]
        size_bytes = _get_tensor_size_bytes(shape, dtype)
        tensors[vi.name] = (shape, dtype, size_bytes)

    # Get tensors from initializers
    for init in graph.initializer:
        shape = list(init.dims) if hasattr(init, "dims") else None
        dtype = int(init.data_type) if hasattr(init, "data_type") else TensorProto.FLOAT

        if shape is None:
            # Enable conversion of models where initializer metadata is incomplete.
            shape = [1]
        size_bytes = _get_tensor_size_bytes(shape, dtype)
        tensors[init.name] = (shape, dtype, size_bytes)

    # Ensure every tensor referenced by graph nodes has metadata.
    # Some exported models omit value_info for intermediate tensors.
    for node in graph.node:
        for tensor_name in list(node.input) + list(node.output):
            if not tensor_name:
                continue
            if tensor_name not in tensors:
                fallback_shape: List[Optional[int]] = [1]
                fallback_dtype = TensorProto.FLOAT
                tensors[tensor_name] = (
                    fallback_shape,
                    fallback_dtype,
                    _get_tensor_size_bytes(fallback_shape, fallback_dtype),
                )

    return tensors


def _layout_tensors(
    tensors: Dict[str, Tuple[Optional[List[Optional[int]]], int, int]],
    platform_config: Dict[str, Any],
) -> Dict[str, int]:
    """
    Layout tensors across devices from the platform's memory_maps.
    Returns a dict mapping tensor name to base address.
    """
    # Extract memory ranges from platform config
    memory_maps = platform_config.get("memory_maps")
    if not memory_maps:
        raise ValueError("No memory maps found in platform configuration")

    # Get ranges from first memory_map entry
    # TODO handle multiple memory maps
    ranges = memory_maps[0].get("ranges")
    if not ranges:
        raise ValueError("No memory ranges found in platform configuration")

    # Track current address for each device
    device_addresses: Dict[str, int] = {}

    # Initialize starting addresses for each device
    for range_info in ranges:
        device = range_info.get("device", "hbm0")
        base_str = range_info.get("base_address", "0x0")
        # Parse base address (handle strings like "0x1_0000_0000" or "16GB")
        if isinstance(base_str, str):
            if "GB" in base_str.upper():
                base = int(base_str.upper().replace("GB", "")) * 1024 * 1024 * 1024
            elif "MB" in base_str.upper():
                base = int(base_str.upper().replace("MB", "")) * 1024 * 1024
            elif "KB" in base_str.upper():
                base = int(base_str.upper().replace("KB", "")) * 1024
            else:
                base = int(base_str.replace("_", ""), 0)
        else:
            base = int(base_str)

        if device not in device_addresses:
            device_addresses[device] = base

    # Round-robin allocation across devices
    device_list = list(device_addresses.keys())
    if not device_list:
        raise ValueError("No devices available for tensor allocation")

    # Sort tensors by size (largest first) for better packing
    sorted_tensors = sorted(
        tensors.items(), key=lambda x: x[1][2], reverse=True  # Sort by size
    )

    tensor_addresses: Dict[str, int] = {}
    current_device_index = 0
    for tensor_name, (_shape, _dtype, size_bytes) in sorted_tensors:
        # Use round-robin to select device
        device = device_list[current_device_index]
        current_device_index = (current_device_index + 1) % len(device_list)

        # Allocate tensor at current address for this device
        addr = device_addresses[device]
        tensor_addresses[tensor_name] = addr

        # Advance address (align to 64-byte boundary)
        aligned_size_bytes = ((size_bytes + 63) // 64) * 64
        device_addresses[device] += aligned_size_bytes

    return tensor_addresses


def _dtype_to_string(dtype: int) -> str:
    """Convert ONNX dtype to string representation."""
    dtype_map: Dict[int, str] = {
        TensorProto.FLOAT: "fp32",
        TensorProto.FLOAT16: "fp16",
        TensorProto.BFLOAT16: "bf16",
        TensorProto.DOUBLE: "fp64",
        TensorProto.INT8: "int8",
        TensorProto.INT16: "int16",
        TensorProto.INT32: "int32",
        TensorProto.INT64: "int64",
        TensorProto.UINT8: "uint8",
        TensorProto.UINT16: "uint16",
        TensorProto.UINT32: "uint32",
        TensorProto.UINT64: "uint64",
        TensorProto.BOOL: "bool",
    }
    return dtype_map.get(dtype, "fp32")  # Default to fp32 if unknown


def _map_onnx_op_to_gwr_op(op_type: str) -> str:
    """Map ONNX operation type to GWR operation type."""
    op_map = {
        "Add": "add",
        "Sub": "add",
        "Mul": "add",
        "Div": "add",
        "MatMul": "mul",
        "Conv": "mul",
        "Relu": "mul",
        "Sigmoid": "mul",
        "Tanh": "mul",
        "BatchNormalization": "mul",
        "MaxPool": "mul",
        "AveragePool": "mul",
        "Flatten": "mul",
        "Reshape": "mul",
        "Transpose": "mul",
        "Concat": "mul",
        "Softmax": "mul",
        "Gemm": "mul",
        "Cast": "mul",
        "Constant": "mul",
        "Gather": "mul",
        "GroupQueryAttention": "mul",
        "MatMulNBits": "mul",
        "QMoE": "mul",
        "ReduceSum": "mul",
        "Shape": "mul",
        "SimplifiedLayerNormalization": "mul",
        "SkipSimplifiedLayerNormalization": "mul",
    }
    return op_map.get(op_type, "add")  # Default to "add" if unknown


def _calculate_num_ops(
    tensors: Dict[str, Tuple[Optional[List[Optional[int]]], int, int]],
    output_name: str,
) -> int:
    """
    Calculate the number of operations based on the output tensor.

    For simplicity, we use the number of elements in the output tensor.
    """
    if output_name not in tensors:
        raise ValueError(f"Output tensor '{output_name}' not found in tensors")

    shape, _dtype, _size_bytes = tensors[output_name]
    if shape is None:
        raise ValueError(
            f"Output tensor '{output_name}' has unknown shape; cannot calculate num ops"
        )

    # Calculate number of elements
    num_elements = _get_tensor_num_elements(shape)

    return max(1, num_elements)  # At least 1 operation


def _get_pe_sram_bytes(platform_config: Dict[str, Any], pe_name: str) -> int:
    """Get SRAM bytes for a specific PE from platform config."""
    pes = platform_config.get("processing_elements", [])
    for pe in pes:
        if pe["name"] == pe_name:
            config = pe.get("config", {})
            sram_bytes = config.get("sram_bytes")
            if sram_bytes is not None:
                # Handle hex strings like "0x20_0000"
                if isinstance(sram_bytes, str):
                    return int(sram_bytes.replace("_", ""), 0)
                return int(sram_bytes)
    return DEFAULT_SRAM_BYTES


def _split_memory_op(
    base_id: str,
    op_type: str,
    pe_name: str,
    base_addr: int,
    total_size_bytes: int,
    sram_bytes: int,
) -> List[Dict[str, Any]]:
    """
    Split a large memory operation into multiple chunks that fit in SRAM.

    Args:
        base_id: Base identifier for the operations
        op_type: "load" or "store"
        pe_name: Name of the PE
        base_addr: Starting address
        total_size_bytes: Total number of bytes to transfer
        sram_bytes: Maximum SRAM size available

    Returns:
        List of memory operation node dictionaries
    """
    ops: List[Dict[str, Any]] = []
    num_chunks = (total_size_bytes + sram_bytes - 1) // sram_bytes  # Ceiling division

    for chunk_index in range(num_chunks):
        offset = chunk_index * sram_bytes
        chunk_size_bytes = min(sram_bytes, total_size_bytes - offset)

        if num_chunks == 1:
            chunk_id = base_id
        else:
            chunk_id = f"{base_id}_chunk_{chunk_index}"

        ops.append(
            {
                "id": chunk_id,
                "kind": "memory",
                "op": op_type,
                "pe": pe_name,
                "config": {
                    "addr": base_addr + offset,
                    "num_bytes": chunk_size_bytes,
                },
            }
        )

    return ops


def onnx_to_gwr_timetable(
    model: Any,
    platform_config: Dict[str, Any],
) -> Dict[str, Any]:
    """
    Convert an ONNX model to a GWR timetable format.

    Args:
        model: ONNX model
        platform_config: Platform configuration dict

    Returns:
        Dictionary containing the GWR timetable
    """
    graph = model.graph

    # Collect all tensors and their sizes
    tensors = _collect_tensors(model)

    # Layout tensors across devices
    tensor_addresses = _layout_tensors(tensors, platform_config)

    # Get list of PEs from platform for round-robin allocation
    pes = platform_config.get("processing_elements", [])
    if not pes:
        raise ValueError("No processing elements found in platform configuration")

    pe_names = [pe["name"] for pe in pes]

    # Generate nodes and edges
    nodes: List[Dict[str, Any]] = []
    edges: List[Dict[str, Any]] = []

    # Process each ONNX node
    current_pe_index = 0
    for onnx_node_index, onnx_node in enumerate(graph.node):
        if not onnx_node.name:
            raise ValueError(f"Node at index {onnx_node_index} is missing a name")
        node_name = onnx_node.name

        # Assign PE using round-robin
        pe_name = pe_names[current_pe_index]
        current_pe_index = (current_pe_index + 1) % len(pe_names)

        # Get SRAM size for this PE
        sram_bytes = _get_pe_sram_bytes(platform_config, pe_name)

        # Generate load operations for each input
        load_node_ids: List[str] = []
        for input_index, input_name in enumerate(onnx_node.input):
            if not input_name:
                # Skip unnamed inputs
                continue

            load_id = f"{node_name}_load_{input_index}"

            addr = tensor_addresses[input_name]
            size_bytes = tensors[input_name][2]

            # Split load into chunks if tensor exceeds SRAM size
            load_ops = _split_memory_op(
                load_id, "load", pe_name, addr, size_bytes, sram_bytes
            )
            nodes.extend(load_ops)

            # Track all load node IDs (including chunks)
            load_node_ids.extend([op["id"] for op in load_ops])

        # Generate compute operation for the node
        compute_id = f"{node_name}_compute"
        gwr_op = _map_onnx_op_to_gwr_op(onnx_node.op_type)

        max_num_ops = 1
        dtype_str = "fp32"  # Default
        for output_name in onnx_node.output:
            if not output_name:
                # Skip unnamed outputs
                continue
            num_ops = _calculate_num_ops(tensors, output_name)
            max_num_ops = max(max_num_ops, num_ops)
            # Use dtype from last non-empty output
            if output_name in tensors:
                dtype_str = _dtype_to_string(tensors[output_name][1])

        nodes.append(
            {
                "id": compute_id,
                "kind": "compute",
                "op": gwr_op,
                "pe": pe_name,
                "config": {
                    "dtype": dtype_str,
                    "num_ops": max_num_ops,
                },
            }
        )

        # Create edges from loads to compute
        for load_id in load_node_ids:
            edges.append(
                {
                    "from": load_id,
                    "to": compute_id,
                    "kind": "data",
                }
            )

        # Generate store operations for each output
        for output_index, output_name in enumerate(onnx_node.output):
            if not output_name:
                # Skip unnamed outputs
                continue

            store_id = f"{node_name}_store_{output_index}"

            addr = tensor_addresses[output_name]
            size_bytes = tensors[output_name][2]

            # Split store into chunks if tensor exceeds SRAM size
            store_ops = _split_memory_op(
                store_id, "store", pe_name, addr, size_bytes, sram_bytes
            )
            nodes.extend(store_ops)

            # Create edges from compute to all store chunks
            for store_op in store_ops:
                edges.append(
                    {
                        "from": compute_id,
                        "to": store_op["id"],
                        "kind": "data",
                    }
                )

    return {
        "nodes": nodes,
        "edges": edges,
    }


def main(argv: Optional[List[str]] = None) -> int:
    """Main entry point."""
    p = argparse.ArgumentParser(
        description="Convert an ONNX model to a GWR timetable YAML with platform layout."
    )
    p.add_argument("onnx", type=Path, help="Path to .onnx model file")
    p.add_argument("platform", type=Path, help="Path to platform YAML file")
    p.add_argument(
        "-o",
        "--output",
        type=Path,
        default=None,
        help="Output YAML path (default: stdout)",
    )
    args = p.parse_args(argv)

    # Load ONNX model
    try:
        model = onnx.load(str(args.onnx))  # type: ignore
    except Exception as e:
        print(f"Error loading ONNX model: {e}", file=sys.stderr)
        return 1

    # Load platform configuration
    try:
        with open(args.platform, "r") as f:
            platform_config = yaml.safe_load(f)
    except Exception as e:
        print(f"Error loading platform YAML: {e}", file=sys.stderr)
        return 1

    # Convert to timetable
    try:
        timetable = onnx_to_gwr_timetable(model, platform_config)
    except Exception as e:
        print(f"Error converting ONNX to timetable: {e}", file=sys.stderr)
        return 1

    # Output YAML
    text = yaml.safe_dump(timetable, sort_keys=False, default_flow_style=False)

    if args.output:
        with open(args.output, "w") as f:
            f.write(text)
    else:
        print(text)

    return 0


if __name__ == "__main__":
    sys.exit(main())
