<!-- Copyright (c) 2025 Graphcore Ltd. All rights reserved. -->

# Changelog

All notable changes for each release of this package will be documented in this file.

## [4.0.0](https://github.com/graphcore-research/gwr/releases/tag/sim-fabric-v4.0.0) - 2025-11-27

### Bug Fixes

- *(gwr-models)* [**breaking**] typo in struct name
- ensure progress bar completes on success

### Documentation

- link to hosted logo
- add code example to README
- extending and cleaning up documentation
- add clarifications and fix typos
- add TRAMWAY image
- ensure `cargo doc --document-private-items` can be used
- improve description of commit scope use
- detail the use of cargo-about

### Features

- add support for alternative port names and monitors
- add routed fabric and improve spotter frontend visualisations
- [**breaking**] rebrand as GWR
- add sim-pipe example application
- Add sim-fabric example
- [**breaking**] rebrand as TRAMWAY

### Infrastructure

- install additional build and dev deps when not run as a GitHub Action
- update Prettier to 3.6.2
- add `cargo doc-steam` and `cargo doc-steam-dev` aliases
- add `cargo clippy-strict` alias
- standardise style used across Github Actions YAML
- run cargo semver-checks seperately from other linting
- disable push CI workflow for `pr/` branches

### Miscellaneous Tasks

- prepare for open source release

### Refactor

- move entity behind accessor function/trait
- remove use of Arc and Mutex
- rename packet to frame for EthernetFrames
- move tracker_builder to tramway-track/src/builder
