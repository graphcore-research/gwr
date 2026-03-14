// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

use criterion::criterion_main;

mod ethernet_frame;

criterion_main! {
    ethernet_frame::benches,
}
