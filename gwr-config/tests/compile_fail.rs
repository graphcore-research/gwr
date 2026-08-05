// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

#[test]
fn rejects_invalid_input() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/*.rs");
}
