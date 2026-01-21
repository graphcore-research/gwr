<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# gwr-perfetto

The `gwr-perfetto` package provides access to the [Perfetto] [TracePacket]
object which underpins the traces which it can [visualise].

[Perfetto] can be used to [visualise] [arbitrary data] with the use of
[TrackDescriptor] and [TrackEvent] objects.

[Perfetto]: https://perfetto.dev
[TracePacket]: https://perfetto.dev/docs/reference/trace-packet-proto
[visualise]: https://perfetto.dev/docs/visualization/perfetto-ui
[arbitrary data]: https://perfetto.dev/docs/getting-started/converting
[TrackDescriptor]:
  https://perfetto.dev/docs/reference/trace-packet-proto#TrackDescriptor
[TrackEvent]: https://perfetto.dev/docs/reference/trace-packet-proto#TrackEvent
