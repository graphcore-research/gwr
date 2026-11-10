// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use gwr_models::cache::coherency_manager::CoherenceOp;
use gwr_models::processing_element::operators::dtype::DataType;
use gwr_models::processing_element::task::ComputeOp;
use gwr_timetable::timetable_file::{
    EdgeKind, EdgeSection, NodeSection, TensorConfigSection, TensorViewSection, TimetableFile,
};

use super::Simulation;
use super::platform::{self, BANK_CAPACITY, BANKS, PES, bank_base, endpoint_name};

const LANES: usize = PES / 2;

/// Configuration for the chain of `Y = X * W_left + X * W_right` computation
/// blocks to be performed by a given pair of PEs.
///
/// `X` is `MxN`, and `W_left` and `W_right` are `NxN`. This means `Y` is also
/// `MxN`, can can be used as `X` in the next block without reshaping.
#[derive(Clone, Copy)]
struct LaneConfig {
    m: usize,
    n: usize,
    blocks: usize,
    expected_sram_partitions: usize,
    require_cache_hits: bool,
}

const SMALL: LaneConfig = LaneConfig {
    m: 32,
    n: 32,
    blocks: 4,
    // All working sets should fit in PE SRAM.
    expected_sram_partitions: 1,
    // Activations, weights and GEMM outputs are much smaller than PE L1, so
    // some reuse should occur.
    require_cache_hits: true,
};

const SRAM_PARTITIONED: LaneConfig = LaneConfig {
    m: 128,
    n: 64,
    blocks: 2,
    // All working sets should exceed PE SRAM.
    expected_sram_partitions: 2,
    // Activations, weights and GEMM outputs combined are significantly larger
    // than PE L1.
    require_cache_hits: false,
};

fn lane_config(lane: usize) -> LaneConfig {
    if lane.is_multiple_of(2) {
        SMALL
    } else {
        SRAM_PARTITIONED
    }
}

pub(super) fn setup(name: &'static str) -> Simulation {
    Simulation::new(name, &platform::config(), timetable_file(), validate)
}

fn validate(simulation: &Simulation) {
    let name = simulation.name;
    let mut small_cache_hits = 0;

    for index in 0..PES {
        let pe_name = endpoint_name("pe", index);
        let pe = simulation.platform.pe(&pe_name).unwrap();

        let LaneConfig {
            m,
            n,
            blocks,
            expected_sram_partitions: partitions,
            require_cache_hits: expect_cache_hits,
        } = lane_config(index / 2);

        let activation_bytes = m * n * 4;
        let weight_bytes = n * n * 4;

        // Each PE executes one GEMM per block and half the lane's joins.
        // M partitioning reloads the complete RHS weight for each partition.
        let reads = blocks * (activation_bytes + partitions * weight_bytes)
            + (blocks / 2) * 2 * activation_bytes;
        let writes = (blocks + blocks / 2) * activation_bytes;
        let operations = blocks * m * n * (2 * n - 1) + (blocks / 2) * m * n;

        assert_eq!(
            pe.total_graph_nodes(),
            blocks + blocks / 2,
            "{name}: {pe_name}"
        );
        assert_eq!(pe.bytes_consumed(), reads, "{name}: {pe_name}");
        assert_eq!(pe.bytes_produced(), writes, "{name}: {pe_name}");
        assert_eq!(pe.total_flops().unwrap(), operations, "{name}: {pe_name}");

        let cache = simulation
            .platform
            .cache(&endpoint_name("l1", index))
            .unwrap();
        if expect_cache_hits {
            small_cache_hits += cache.num_hits();
        }
        assert!(
            cache.num_misses() > 0,
            "{name}: {pe_name} has no cache misses"
        );

        let traffic = cache.traffic_snapshot();
        for op in [CoherenceOp::SharedRead, CoherenceOp::ExclusiveWrite] {
            assert!(
                traffic.coherence_sent[op as usize] > 0,
                "{name}: {pe_name} cache never sent {op:?}"
            );
        }
        for op in [CoherenceOp::GrantShared, CoherenceOp::GrantExclusive] {
            assert!(
                traffic.coherence_received[op as usize] > 0,
                "{name}: {pe_name} cache never received {op:?}"
            );
        }
    }

    // Allocating coherent writes compete with weights for cache space. The
    // larger lanes can legitimately stream through their L1s without any hits.
    assert!(small_cache_hits > 0, "{name}: no small-lane cache reuse");

    for bank in 0..BANKS {
        validate_bank(simulation, bank);
    }
}

fn validate_bank(simulation: &Simulation, bank: usize) {
    let name = simulation.name;
    let memory = simulation.platform.memory(&format!("hbm{bank}")).unwrap();

    assert!(memory.bytes_read() > 0, "{name}: HBM {bank} has no reads");
    assert!(
        memory.bytes_written() > 0,
        "{name}: HBM {bank} has no writes"
    );

    let manager = simulation
        .platform
        .coherency_manager(&format!("cm{bank}"))
        .unwrap();
    for op in [
        CoherenceOp::SharedRead,
        CoherenceOp::ExclusiveWrite,
        CoherenceOp::InvalidateAck,
        CoherenceOp::Writeback,
    ] {
        assert!(
            manager.received_coherence_op_count(op) > 0,
            "{name}: HBM {bank} directory never received {op:?}"
        );
    }
    for (request, response) in [
        (CoherenceOp::SharedRead, CoherenceOp::GrantShared),
        (CoherenceOp::ExclusiveWrite, CoherenceOp::GrantExclusive),
        (CoherenceOp::InvalidateAck, CoherenceOp::Invalidate),
    ] {
        assert_eq!(
            manager.received_coherence_op_count(request),
            manager.sent_coherence_op_count(response),
            "{name}: HBM {bank} directory has outstanding {response:?}"
        );
    }
}

struct GraphBuilder {
    file: TimetableFile,
    next_offsets: [u64; BANKS],
}

impl GraphBuilder {
    fn tensor(&mut self, id: &str, shape: &[usize], bank: usize) {
        let offset = &mut self.next_offsets[bank];
        let addr = bank_base(bank) + *offset;
        let bytes = u64::try_from(shape.iter().product::<usize>() * 4).unwrap();
        // Every tensor has fresh storage; outputs do not overwrite inputs or
        // reuse earlier output buffers.
        *offset += bytes.next_multiple_of(64);
        assert!(*offset <= BANK_CAPACITY);
        self.file.nodes.push(NodeSection::Tensor {
            id: id.to_owned(),
            config: TensorConfigSection {
                addr,
                dtype: DataType::Fp32,
                shape: shape.to_vec(),
            },
        });
    }

    fn edge(&mut self, from: impl Into<String>, to: impl Into<String>) {
        self.file.edges.push(EdgeSection {
            from: from.into(),
            to: to.into(),
            kind: EdgeKind::Data,
        });
    }

    fn compute(
        &mut self,
        id: &str,
        op: ComputeOp,
        pe: usize,
        input_views: Vec<Option<TensorViewSection>>,
        output_views: Vec<Option<TensorViewSection>>,
    ) {
        self.file.nodes.push(NodeSection::Compute {
            id: id.to_owned(),
            op,
            pe: Some(endpoint_name("pe", pe)),
            input_views,
            output_views,
        });
    }
}

fn half_view(m: usize, n: usize, branch: usize) -> Option<TensorViewSection> {
    Some(TensorViewSection {
        offsets: vec![branch * m, 0],
        shape: vec![m, n],
    })
}

/// Build FP32 GEMM/Add chains for thirty PE pairs. Each block runs two
/// GEMMs from the same input, then joins their results with Add. Small and
/// SRAM-partitioned lanes alternate, sharing four weight tensors per size.
fn timetable_file() -> TimetableFile {
    let mut builder = GraphBuilder {
        file: TimetableFile {
            nodes: Vec::new(),
            edges: Vec::new(),
        },
        next_offsets: [0; BANKS],
    };

    // Create tensors for the shared weight matrices of each chain size, one per
    // bank.
    for config in [SMALL, SRAM_PARTITIONED] {
        for bank in 0..BANKS {
            let n = config.n;
            builder.tensor(&format!("weight{n}_{bank}"), &[n, n], bank);
        }
    }

    // Create tensors for the input matrix of each chain.
    for lane in 0..LANES {
        let LaneConfig { m, n, .. } = lane_config(lane);
        builder.tensor(&format!("x{lane}_0"), &[m, n], lane % BANKS);
    }

    for lane in 0..LANES {
        let LaneConfig { m, n, blocks, .. } = lane_config(lane);
        for block in 0..blocks {
            // Create tensors for the matmul and elementwise add outputs.
            // The input of this block is the output of the previous block.
            let input = format!("x{lane}_{block}");
            let joined = format!("branches{lane}_{block}");
            let output = format!("x{lane}_{}", block + 1);
            builder.tensor(&joined, &[2 * m, n], (lane + block + 1) % BANKS);
            builder.tensor(&output, &[m, n], (lane + block + 2) % BANKS);

            // Create the matmul operations.
            for branch in 0..2 {
                let gemm = format!("gemm{lane}_{block}_{branch}");
                builder.compute(
                    &gemm,
                    ComputeOp::Gemm,
                    2 * lane + branch,
                    vec![None, None],
                    vec![half_view(m, n, branch)],
                );
                builder.edge(&input, format!("{gemm}.0"));
                builder.edge(
                    format!("weight{n}_{}", (lane + branch) % BANKS),
                    format!("{gemm}.1"),
                );
                builder.edge(gemm, &joined);
            }

            // Create the elementwise add operations.
            let add = format!("add{lane}_{block}");
            builder.compute(
                &add,
                ComputeOp::Add,
                2 * lane + block % 2,
                vec![half_view(m, n, 0), half_view(m, n, 1)],
                vec![None],
            );
            builder.edge(&joined, format!("{add}.0"));
            builder.edge(&joined, format!("{add}.1"));
            builder.edge(add, output);
        }
    }
    builder.file
}
