# Copyright (c) 2026 Graphcore Ltd. All rights reserved.

"""Benchmark real simulated GEMMs, retaining inputs, references and raw timings."""

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import statistics
import subprocess
import time

import numpy as np
import torch


def prepare(directory, size, tile_size=None):
    directory.mkdir(parents=True, exist_ok=True)
    rng = np.random.default_rng(20261103 + size)
    a = rng.standard_normal((size, size), dtype=np.float32)
    b = rng.standard_normal((size, size), dtype=np.float32) / math.sqrt(size)
    expected = (torch.from_numpy(a).double() @ torch.from_numpy(b).double()).float().numpy()
    for name, values in [("a", a), ("b", b), ("expected", expected)]:
        values.astype("<f4", copy=False).tofile(directory / (name + ".f32"))
    address = 65536
    tensor_bytes = size * size * 4
    addresses = [address + i * ((tensor_bytes + 65535) // 65536 * 65536) for i in range(3)]
    nodes = [
        {"id": name, "kind": "tensor", "config": {
            "addr": addr, "dtype": "fp32", "shape": [size, size],
        }} for name, addr in zip(["A", "B", "Y"], addresses)
    ]
    nodes.append({"id": "gemm", "kind": "compute", "pe": "pe0", "program": {
        "inputs": ["A", "B"], "outputs": ["Y"], "assignments": [{
            "output": "Y", "axes": ["m", "n"], "expr": {"matrix_mac": {
                "over": ["k"], "args": [
                    {"read": {"tensor": "A", "indices": ["m", "k"]}},
                    {"read": {"tensor": "B", "indices": ["k", "n"]}},
                ],
            }},
        }],
    }})
    if tile_size:
        nodes[-1]["tiling"] = {"sizes": {"m": tile_size, "n": tile_size, "k": min(256, size)}}
    (directory / "timetable.json").write_text(json.dumps({"nodes": nodes, "edges": [
        {"from": "A", "to": "gemm.0", "kind": "data"},
        {"from": "B", "to": "gemm.1", "kind": "data"},
        {"from": "gemm.0", "to": "Y", "kind": "data"},
    ]}))
    (directory / "data.json").write_text(json.dumps({
        "inputs": [{"address": addresses[0], "file": "a.f32"},
                   {"address": addresses[1], "file": "b.f32"}],
        "checks": [{"address": addresses[2], "file": "expected.f32"}],
        "atol": 2e-5, "rtol": 2e-4,
    }))
    # The complete buffers fit in SRAM; no tile-shape changes between modes.
    platform = f"""
memory_maps:
  - name: main
    devices: [mem]
operation_classes:
  matrix: {{operations: [matrix_mac], arity: 2, flops_per_op: 2}}
processing_elements:
  - name: pe0
    memory_map: main
    config:
      sram_bytes: {max(1048576, tensor_bytes * 4)}
      lsu_access_bytes: 65536
      num_active_requests: 16
      compute:
        engines:
          matrix:
            capabilities: [{{class: matrix, input_dtypes: [[fp32, fp32]], ops_per_tick: 256}}]
memories:
  - name: mem
    kind: hbm
    base_address: 0
    config: {{capacity_bytes: 1GiB, bw_bytes_per_tick: 65536}}
connections:
  - connect: [pe.pe0, mem.mem]
"""
    (directory / "platform.yaml").write_text(platform)
    return torch.from_numpy(a), torch.from_numpy(b)


def run(binary, directory, functional, timeout):
    command = [str(binary), "--platform", str(directory / "platform.yaml"),
               "--timetable", str(directory / "timetable.json"),
               "--error-mermaid", str(directory / "error.mmd")]
    if functional:
        command += ["--functional-data", str(directory / "data.json")]
    started = time.perf_counter()
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return {"status": "timeout", "timeout_seconds": timeout}
    elapsed = time.perf_counter() - started
    if result.returncode:
        raise RuntimeError(f"{binary} exited {result.returncode}\n" + result.stdout + result.stderr)
    engine = re.search(r"completed in ([0-9.]+)s", result.stdout)
    ticks = re.search(r"Time now ([0-9.]+)ns", result.stdout)
    error = re.search(r"max absolute error ([0-9.eE+-]+)", result.stdout)
    if functional and error is None:
        raise RuntimeError("Functional run did not check its reference outputs")
    return {"status": "ok", "wall_seconds": elapsed,
            "engine_seconds": float(engine[1]), "simulated_ns": float(ticks[1]),
            "max_absolute_error": float(error[1]) if error else None}


def host_comparison(a, b, repeats):
    results = {}
    reference = (a.double() @ b.double()).float()
    for device in ["cpu"] + (["mps"] if torch.backends.mps.is_available() else []):
        left, right = a.to(device), b.to(device)
        synchronize = torch.mps.synchronize if device == "mps" else lambda: None
        left @ right
        synchronize()
        compute, total = [], []
        for _ in range(repeats):
            started = time.perf_counter()
            output = left @ right
            synchronize()
            compute.append(time.perf_counter() - started)
            started = time.perf_counter()
            output = (a.to(device) @ b.to(device)).cpu()
            synchronize()
            total.append(time.perf_counter() - started)
        if not torch.allclose(output, reference, atol=2e-5, rtol=2e-4):
            raise RuntimeError(f"{device} output differs from the independent f64 reference")
        results[device] = {"kernel_seconds": compute, "with_transfers_seconds": total,
                           "kernel_median_seconds": statistics.median(compute),
                           "with_transfers_median_seconds": statistics.median(total),
                           "max_absolute_error": (output - reference).abs().max().item()}
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--default-binary", type=Path)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--sizes", type=int, nargs="+", default=[128, 512, 1024, 2048])
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--timeout", type=float, default=120)
    parser.add_argument("--host-only", action="store_true")
    parser.add_argument("--cpu-threads", type=int, default=torch.get_num_threads())
    parser.add_argument("--tile-size", type=int, help="Output tile extent; default is the full output")
    args = parser.parse_args()
    if not args.host_only and args.binary is None:
        parser.error("--binary is required unless --host-only is set")
    if args.repeats < 1 or args.cpu_threads < 1 or any(size < 1 or size > 8192 for size in args.sizes):
        parser.error("repeats/threads must be positive and sizes must be in 1..8192")
    if args.tile_size is not None and args.tile_size < 1:
        parser.error("tile size must be positive")
    if not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error("timeout must be finite and positive")
    torch.set_num_threads(args.cpu_threads)
    args.out.mkdir(parents=True, exist_ok=True)
    cpu_model = platform.processor()
    if platform.system() == "Darwin":
        model = subprocess.run(
            ["/usr/sbin/sysctl", "-n", "machdep.cpu.brand_string"], capture_output=True, text=True
        )
        if model.returncode == 0:
            cpu_model = model.stdout.strip()
    binaries = {name: {"path": str(path.resolve()), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
                for name, path in [("functional", args.binary), ("default", args.default_binary)] if path}
    report = {"platform": platform.platform(), "cpu_model": cpu_model, "logical_cpus": os.cpu_count(),
              "binaries": binaries, "torch": torch.__version__, "numpy": np.__version__,
              "torch_cpu_threads": torch.get_num_threads(), "tile_size": args.tile_size,
              "mps_available": torch.backends.mps.is_available(), "results": []}
    for size in args.sizes:
        directory = args.out.resolve() / str(size)
        a, b = prepare(directory, size, args.tile_size)
        entry = {"size": size, "flops": 2 * size ** 3}
        if args.host_only:
            entry["host"] = host_comparison(a, b, args.repeats)
        else:
            modes = [("timing", args.binary, False), ("functional", args.binary, True)]
            if args.default_binary:
                modes.insert(0, ("default", args.default_binary, False))
            for name, binary, functional in modes:
                samples = []
                for _ in range(args.repeats):
                    sample = run(binary.resolve(), directory, functional, args.timeout)
                    samples.append(sample)
                    print(size, name, sample, flush=True)
                    if sample["status"] == "timeout":
                        break
                entry[name] = samples
            ticks = {s["simulated_ns"] for name, _, _ in modes
                     for s in entry[name] if s["status"] == "ok"}
            if len(ticks) != 1:
                raise RuntimeError(f"Simulation time changed between modes: {ticks}")
        report["results"].append(entry)
        (args.out / "results.json").write_text(json.dumps(report, indent=2) + "\n")
        print("Completed", size, flush=True)


if __name__ == "__main__":
    main()
