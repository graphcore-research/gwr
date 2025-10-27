<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# GWR Spotter

`gwr-spotter` is a utility designed to provide an interactive TUI (Textual User
Interface) for working with log/bin files produced by [`gwr_track`].

It is based on the [`ratatui`] library.

## Launching

The best way to launch GWR Spotter is using `cargo run` to ensure that all
dependencies are up to date. For example, in order to launch it and open the
`trace.bin` file use:

```bash
cargo run --release --bin gwr-spotter -- --bin trace.bin
```

## Commands

The most help command to know about is the help as that should contain the
latest up to date command. Use the `?` key to open the help and press `Esc` to
close that view.

## Frontend

There is a web-based frontend that can be used to interact with `gwr-spotter`.
In order to use the frontend you need to run a server on a local port:

```bash
cd gwr-spotter/frontend ; python3 -m http.server 9991
```

and then simply open `http://localhost:9991` in a web browser on the same
machine. Whenever `gwr-spotter` is active you can view the structure of the
model. Any element in the web view that you select will be selected in the TUI.

### Views

Note that there are a number of different views of the model that are available
within the frontend. The default is a sunburst view which shows the hierarchy.
Under the menu on the left there is also a force-tree view that shows how the
components are connected.

[`ratatui`]: https://docs.rs/ratatui/latest/ratatui/
[`gwr_track`]: ../gwr_track/chapter.md
