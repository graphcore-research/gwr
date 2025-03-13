# Documentation

This chapter describes how the STEAM [engine] and accompanying libraries are
documented.

{{#include ../../../README.md:dev_docs}}

This user guide is written using [`mdbook`]. However, because these packages are
not released via crates.io the usual Rust playground integration is disabled.

As use of the `mdbook test` command does not lend itself well to testing crates
directly from a local workspace all Rust source within the developer guide is
instead tested using the [mdBook-Keeper] plugin. This plugin tests all included
code snippets during the [mdBook] build process.

A "wrapper" test within the Rust source of the steam_developer_guide library is
used to ensure that the [mdBook] build process is tested whenever `cargo test`
is run at the workspace level.

{{#include ../../../README.md:api_docs}}

Building this developer guide also generates the [API documentation].

{{#include ../links_depth1.md}}
