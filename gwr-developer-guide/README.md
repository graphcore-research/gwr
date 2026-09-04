<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# gwr-developer-guide

`gwr-developer-guide` contains the mdBook source, theme, and build test for the
GWR Developer Guide.

By making `gwr-developer-guide` a Rust package that is included in the workspace
it can be tested in a similar way to the Rust documentation. The `mdbook_build`
test aims to ensure that the mdBook build process remains warning and error
free, which should avoid the book containing Rust code examples that do not
compile or any broken links.

Unlike normal Cargo tests, the generated output is written into the source tree.
The rendered mdBook uses the `book` directory, and the temporary files used to
check compilation of code examples use the `doctest_cache` directory.

As the workspace target directory cannot be used in this case, due to an issue
experienced with mdbook-keeper, `cargo clean` will not remove these build files.
When `mdbook build` is invoked directly from the command line, the generated
output is also written to the `book` and `doctest_cache` directories.
