<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# Components

{{#include ../../../gwr-components/README.md:intro}}

[`Engine`]: ../gwr_engine/chapter.md
[`ports`]: ../components/ports.md

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

[Add ports]: ../components/ports.md
[Create a struct]: ../components/create_a_struct.md
[Create subcomponents]: ../components/add_subcomponents.md
[Design the component]: ../components/design.md
[Implement any custom functionality]: ../components/custom_functionality.md
