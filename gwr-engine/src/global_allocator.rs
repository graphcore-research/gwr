// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;
