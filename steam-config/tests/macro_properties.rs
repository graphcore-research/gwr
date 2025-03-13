// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

#[test]
fn build_failures() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/macro_properties/name_only.rs");
    tests.compile_fail("tests/macro_properties/missing_path.rs");
    tests.compile_fail("tests/macro_properties/unsupported.rs");
}
