<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# onnx-opset-dump

Small CLI for decoding an ONNX `OperatorSetProto` protobuf and rendering it as
YAML.

## Official ONNX schema sources

These files were downloaded from the ONNX repository:

- <https://github.com/onnx/onnx/blob/main/onnx/onnx-operators.proto>
- <https://github.com/onnx/onnx/blob/main/onnx/onnx.proto>

Raw download URLs:

- <https://raw.githubusercontent.com/onnx/onnx/main/onnx/onnx-operators.proto>
- <https://raw.githubusercontent.com/onnx/onnx/main/onnx/onnx.proto>

## Usage

```bash
cargo run -p onnx-opset-dump -- path/to/operatorset.pb
```

Write YAML to a file:

```bash
cargo run -p onnx-opset-dump -- path/to/operatorset.pb --out operatorset.yaml
```
