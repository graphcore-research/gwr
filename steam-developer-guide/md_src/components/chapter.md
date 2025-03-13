<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# Components

Simulation components are the basic building blocks of any STEAM model.

The STEAM `Engine` runs components that are connected together using `ports`.

The `steam_components` library provides a collection of connectable component
primitives to be used when building models. These are not intended to include or
model Graphcore IP.

## Creating new components

Components are designed to be composable and connectable simulation blocks. When
creating a new one it is important to consider all of the following steps:

1. [Design the component]
1. [Create a struct]
1. [Add ports]
1. [Create subcomponents]
1. [Implement any custom functionality]
1. Provide default implementations for other methods

This documentation will take you through designing a custom component that will
be used to drop a random number of objects that pass through it.

{{#include ../links_depth1.md}}
