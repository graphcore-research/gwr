<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# Rust

[Rust] is the chosen language for TRAMWAY for a number of reasons.

1. Strongly typed.
   - Helps enforce more rigorous software design.
2. Fast compiled code.
   - Should produce applications that run as quickly as possible.
3. Prevents unsafe code.
   - Improves the quality and robustness of our models. Up front cost that
     should save time overall.
4. Good [`async`] programming support.
   - Rust is still evolving around async support, but provides what we need now.
5. Excellent build system.
   - Integrated documentation, tests, benchmarking.
6. Easy integration with 3rd party libraries.
   - Crates provide access to many high-quality 3rd party libraries.

The objective is to allow us to write larger models where the tools provide more
compile-time checks.

[`async`]: https://rust-lang.github.io/async-book/
[Rust]: https://www.rust-lang.org
