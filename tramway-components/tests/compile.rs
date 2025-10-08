// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

#[test]
fn compile() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/fail/*.rs");
}
