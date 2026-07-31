// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

#[test]
fn rejects_invalid_macro_input() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/*.rs");
}
