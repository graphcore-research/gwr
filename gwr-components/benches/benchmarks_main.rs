// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use criterion::criterion_main;

mod components;

criterion_main! {
    components::small_benches, components::large_benches,
}
