<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# TRAMWAY Spotter

`tramway-spotter` is a utility designed to provide an interactive TUI (Textual
User Interface) for working with log/bin files produced by [`tramway_track`].

It is based on the [`ratatui`] library.

## Launching

The best way to launch TRAMWAY Spotter is using `cargo run` to ensure that all
dependencies are up to date. For example, in order to launch it and open the
`trace_full.bin` file use:

```bash
cargo run --release --bin tramway-spotter -- --bin trace_full.bin
```

## Commands

The most help command to know about is the help as that should contain the
latest up to date command. Use the `?` key to open the help and press `Esc` to
close that view.

[`ratatui`]: https://docs.rs/ratatui/latest/ratatui/
[`tramway_track`]: ../tramway_track/chapter.md
