#!/usr/bin/env python3

# Copyright (c) 2026 Graphcore Ltd. All rights reserved.

# A script to generate a platform with ProcessingElements and Memories attached to
# a 2D Fabric. The ProcessingElements can be connected through L1/L2 caches.

import argparse
import sys

# Use raumel YAML because it supports YAML anchors
from ruamel.yaml import YAML
from ruamel.yaml.comments import CommentedMap, CommentedSeq


class PeIdGen:
    """
    A Generator that returns all (column, row) pairs that have PEs attached
    """
    def __init__(self, args):
        self.num_columns = args.num_columns
        self.num_rows = args.num_rows
        num_memories = args.num_hbms
        self.pe_id = 0
        self.max_pe_id = (self.num_columns * self.num_rows) - num_memories

    def __iter__(self):
        return self

    def __next__(self):
        if self.pe_id < self.max_pe_id:
            pe_id, self.pe_id = self.pe_id, self.pe_id+1
            (column, row) = (pe_id // self.num_rows, pe_id % self.num_rows)
            return (column, row)
        raise StopIteration()


class HbmIdGen:
    """
    A Generator that returns all (column, row) pairs that have HBMs attached
    """
    def __init__(self, args):
        self.num_columns = args.num_columns
        self.num_rows = args.num_rows
        num_memories = args.num_hbms
        self.max_mem_id = (self.num_columns * self.num_rows)
        self.mem_id = (self.num_columns * self.num_rows) - num_memories

    def __iter__(self):
        return self

    def __next__(self):
        if self.mem_id < self.max_mem_id:
            mem_id, self.mem_id = self.mem_id, self.mem_id+1
            (column, row) = (mem_id // self.num_rows, mem_id % self.num_rows)
            return (column, row)
        raise StopIteration()


# Function to standardise entity names
def create_name(prefix, column, row):
    return f"{prefix}_{column}_{row}"


FABRIC_NAME = "fabric0"

def build_fabrics(args):
    """
    Build the Fabrics (currently only supports a single fabric).
    """
    fabrics = CommentedSeq()
    fabric = CommentedMap(
        name = FABRIC_NAME,
        kind = args.fabric_model,
        columns = args.num_columns,
        rows = args.num_rows,
        routing = "column-first"
    )
    fabrics.append(fabric)

    return fabrics


LINE_SIZE_BYTES = 32

def build_cache(name, kib, bytes_per_cycle, num_ways, latency):
    num_sets = (kib * 1024) // num_ways // LINE_SIZE_BYTES
    return CommentedMap(
        name = name,
        bw_bytes_per_cycle = bytes_per_cycle,
        line_size_bytes = LINE_SIZE_BYTES,
        num_ways = num_ways,
        num_sets = num_sets,
        delay_ticks = latency
    )


def build_caches(args):
    """
    Build all the L1/L2 caches specified
    """
    caches = CommentedSeq()

    if args.l1_kib == 0 and args.l2_kib == 0:
        return

    for (column, row) in PeIdGen(args):
        if args.l1_kib != 0:
            name = create_name("l1", column, row)
            l1 = build_cache(
                name, args.l1_kib, args.l1_bytes_per_cycle, args.l1_num_ways, args.l1_latency)
            caches.append(l1)

        if args.l2_kib != 0:
            name = create_name("l2", column, row)
            l2 = build_cache(
                name, args.l2_kib, args.l2_bytes_per_cycle, args.l2_num_ways, args.l2_latency)
            caches.append(l2)

    return caches


def build_memories(args):
    base = args.hbm_base
    size = args.hbm_size

    memories = CommentedSeq()
    for i in range(0, args.num_hbms):
        mem = CommentedMap(
            name = f"hbm{i}",
            kind = "hbm",
            base_address = f'0x{base:x}',
            capacity_bytes = f'0x{size:x}',
            delay_ticks = 10,
        )
        base += size
        memories.append(mem)
    return memories


def build_connections(args):
    connections = CommentedSeq()
    for (column, row) in PeIdGen(args):
        # Create a list of entities in the chain from PE -> Fabric
        entities = [f"pe.{create_name('pe', column, row)}"]
        if args.l1_kib != 0:
            entities.append(f"cache.{create_name('l1', column, row)}")
        if args.l2_kib != 0:
            entities.append(f"cache.{create_name('l2', column, row)}")
        entities.append(f"fabric.{FABRIC_NAME}@({column},{row})")

        # Connect each pair in the chain
        for i in range(len(entities) - 1):
            connections.append(CommentedMap(
                connect = [entities[i], entities[i+1]]
            ))

    for (i, (column, row)) in enumerate(HbmIdGen(args)):
        connections.append(CommentedMap(
            connect = [f"mem.hbm{i}", f"fabric.{FABRIC_NAME}@({column},{row})"]
        ))

    return connections


def build_pe_config(args):
    pe_config = CommentedMap(
        num_active_requests = args.pe_active_requests,
        lsu_access_bytes = args.pe_lsu_access_bytes,
        sram_bytes = args.pe_sram_bytes,
        adds_per_tick = args.pe_adds_per_tick,
        muls_per_tick = args.pe_muls_per_tick
    )
    pe_config.yaml_set_anchor("default_pe_config", always_dump=True)
    return pe_config


def build_memory_maps(args):
    ranges = CommentedSeq()
    base = args.hbm_base
    size = args.hbm_size
    for mm in range(0, args.num_hbms):
        memory_map = CommentedMap(
            base_address=f"0x{base:x}",
            size_bytes=f"0x{size:x}",
            device=f"hbm{mm}",
        )
        ranges.append(memory_map)
        base += size

    ranges.yaml_set_anchor("pe_memory_map", always_dump=True)
    return ranges


def build_processing_elements(args, ranges, pe_config):
    processing_elements = CommentedSeq()
    for (column, row) in PeIdGen(args):
        pe = CommentedMap(
            name = create_name("pe", column, row),
            memory_map = CommentedMap(
                ranges = ranges
            ),
            config = pe_config
        )
        processing_elements.append(pe)

    return processing_elements


def build_platform(args):
    ranges = build_memory_maps(args)
    pe_config = build_pe_config(args)
    processing_elements = build_processing_elements(args, ranges, pe_config)
    connections = build_connections(args)
    memories = build_memories(args)
    fabrics = build_fabrics(args)
    caches = build_caches(args)
    platform = CommentedMap(
        memory = ranges,
        config = pe_config,
        fabrics = fabrics,
        processing_elements = processing_elements,
        memories = memories,
        caches = caches,
        connections = connections,
    )
    return platform

def write_platform(args, platform, yaml):
    file_name = args.out
    with open(file_name, "w") as file:
        yaml.dump(platform, file)


def parse_args():
    parser = argparse.ArgumentParser(
        description="Build a platform file based around a 2D fabric"
    )

    parser.add_argument(
        "--out",
        type=str,
        default="platform.yaml",
        help="The platform file to write"
    )

    # PE configuration
    parser.add_argument(
        "--pe-active-requests",
        type=int,
        default="8",
        help="Number of outstanding requests the PE LSU supports",
    )
    parser.add_argument(
        "--pe-lsu-access-bytes",
        type=int,
        default="32",
        help="Number of bytes per PE LSU access",
    )
    parser.add_argument(
        "--pe-sram-bytes",
        type=int,
        default=str(1024*1024),
        help="Number of local SRAM bytes in the PE",
    )
    parser.add_argument(
        "--pe-adds-per-tick",
        type=int,
        default="16",
        help="Number of adds the PE can perform per tick",
    )
    parser.add_argument(
        "--pe-muls-per-tick",
        type=int,
        default="4",
        help="Number of muls the PE can perform per tick",
    )

    # Fabric configuration options
    parser.add_argument(
        "--fabric-model",
        type=str,
        default="functional",
        choices=["routed", "functional"],
        help="Which fabric model to use.",
    )
    parser.add_argument(
        "--num-columns",
        type=int,
        default="2",
        help="Number of columns in the fabric",
    )
    parser.add_argument(
        "--num-rows",
        type=int,
        default="2",
        help="Number of rows in the fabric",
    )

    # Memory configuration
    parser.add_argument(
        "--num-hbms",
        type=int,
        default="2",
        help="Number of HBMs",
    )
    # Use default of 1GB memories
    hbm_size = 1024*1024*1024
    parser.add_argument(
        "--hbm-base",
        type=int,
        default=str(hbm_size),
        help="Base address of the HBMs",
    )
    parser.add_argument(
        "--hbm-size",
        type=int,
        default=str(hbm_size),
        help="Size of the HBM in bytes",
    )


    # Cache configuration options
    parser.add_argument(
        "--l1-kib",
        type=int,
        default="0",
        help="L1 cache capacity in kibibyte",
    )
    parser.add_argument(
        "--l1-bytes-per-cycle",
        type=int,
        default="32",
        help="L1 bandwidth in bytes per cycle",
    )
    parser.add_argument(
        "--l1-num-ways",
        type=int,
        default="4",
        help="L1 number of ways per set",
    )
    parser.add_argument(
        "--l1-latency",
        type=int,
        default="5",
        help="L1 latency in clock ticks",
    )
    parser.add_argument(
        "--l2-kib",
        type=int,
        default="0",
        help="L2 cache capacity in kibibyte",
    )
    parser.add_argument(
        "--l2-bytes-per-cycle",
        type=int,
        default="32",
        help="L2 bandwidth in bytes per cycle",
    )
    parser.add_argument(
        "--l2-num-ways",
        type=int,
        default="8",
        help="L2 number of ways per set",
    )
    parser.add_argument(
        "--l2-latency",
        type=int,
        default="20",
        help="L2 latency in clock ticks",
    )
    args = parser.parse_args()

    if args.l2_kib != 0 and args.l1_kib == 0:
        print("Cannot have an L2 cache without an L1 cache")
        sys.exit(1)

    return args


def main():
    args = parse_args()

    yaml = YAML()
    yaml.default_flow_style = False

    platform = build_platform(args)
    write_platform(args, platform, yaml)


if __name__ == "__main__":
    main()
