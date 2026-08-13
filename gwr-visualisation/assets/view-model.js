// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

(() => {
  function toBigInt(value, fallback = 0n) {
    if (value === undefined || value === null || value === "") {
      return fallback;
    }
    return BigInt(value);
  }

  function addressRange(start, bytes) {
    const rangeStart = toBigInt(start);
    return [rangeStart, rangeStart + toBigInt(bytes)];
  }

  function intersectRanges(left, right) {
    const start = left[0] > right[0] ? left[0] : right[0];
    const end = left[1] < right[1] ? left[1] : right[1];
    return end > start ? [start, end] : null;
  }

  function rangeUnionBytes(ranges) {
    const sorted = ranges
      .filter((range) => range && range[1] > range[0])
      .map((range) => [...range])
      .sort((left, right) =>
        left[0] < right[0] ? -1 : left[0] > right[0] ? 1 : 0,
      );
    let total = 0n;
    let merged = null;
    for (const range of sorted) {
      if (!merged) {
        merged = range;
      } else if (range[0] <= merged[1]) {
        if (range[1] > merged[1]) {
          merged[1] = range[1];
        }
      } else {
        total += merged[1] - merged[0];
        merged = range;
      }
    }
    return merged ? total + merged[1] - merged[0] : total;
  }

  function trafficForAccesses(
    accesses,
    selectedLayers = null,
    selectedMemoryRanges = null,
  ) {
    let bytes = 0n;
    let edgeCount = 0;
    for (const access of accesses || []) {
      if (selectedLayers !== null && !selectedLayers.has(access.layer)) {
        continue;
      }
      let edgeBytes = 0n;
      for (const range of access.ranges || []) {
        const accessRange = addressRange(range.addr, range.num_bytes);
        edgeBytes +=
          selectedMemoryRanges === null
            ? accessRange[1] - accessRange[0]
            : rangeUnionBytes(
                selectedMemoryRanges.map((memoryRange) =>
                  intersectRanges(accessRange, memoryRange),
                ),
              );
      }
      if (edgeBytes > 0n) {
        bytes += edgeBytes;
        edgeCount += 1;
      }
    }
    return { bytes, edgeCount };
  }

  function retainedFocus(focusedView, visibleViews) {
    if (focusedView === null) {
      return null;
    }
    const visible =
      visibleViews instanceof Set ? visibleViews : new Set(visibleViews);
    return visible.has(focusedView) ? focusedView : null;
  }

  function contextTensorCount(context) {
    return context?.tensors?.length || 0;
  }

  window.GWR_VISUALISATION_VIEW_MODEL = {
    addressRange,
    intersectRanges,
    rangeUnionBytes,
    trafficForAccesses,
    retainedFocus,
    contextTensorCount,
  };
})();
