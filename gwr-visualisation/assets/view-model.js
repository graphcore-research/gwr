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

  function elementRangeToBytes(start, end, bitsPerElement) {
    return [(start * bitsPerElement) / 8n, (end * bitsPerElement + 7n) / 8n];
  }

  function accessBounds(
    firstElement,
    elementsPerRange,
    strides,
    bitsPerElement,
  ) {
    const lastRangeStart = strides.reduce(
      (start, stride) =>
        start +
        (toBigInt(stride.count) - 1n) * toBigInt(stride.stride_elements),
      firstElement,
    );
    return elementRangeToBytes(
      firstElement,
      lastRangeStart + elementsPerRange,
      bitsPerElement,
    );
  }

  function oddAccessRangeCount(firstElement, strides) {
    let even = firstElement % 2n === 0n ? 1n : 0n;
    let odd = firstElement % 2n === 0n ? 0n : 1n;
    for (const stride of strides) {
      const count = toBigInt(stride.count);
      if (toBigInt(stride.stride_elements) % 2n === 0n) {
        even *= count;
        odd *= count;
        continue;
      }
      const evenCoordinates = (count + 1n) / 2n;
      const oddCoordinates = count / 2n;
      [even, odd] = [
        even * evenCoordinates + odd * oddCoordinates,
        even * oddCoordinates + odd * evenCoordinates,
      ];
    }
    return odd;
  }

  function accessBytes(
    firstElement,
    elementsPerRange,
    strides,
    bitsPerElement,
  ) {
    const rangeCount = strides.reduce(
      (count, stride) => count * toBigInt(stride.count),
      1n,
    );
    if (bitsPerElement % 8n === 0n) {
      return elementsPerRange * (bitsPerElement / 8n) * rangeCount;
    }
    let bytes = ((elementsPerRange + 1n) / 2n) * rangeCount;
    if (elementsPerRange % 2n === 0n) {
      bytes += oddAccessRangeCount(firstElement, strides);
    }
    return bytes;
  }

  function partitionPoint(count, predicate) {
    let first = 0n;
    let length = count;
    while (length > 0n) {
      const half = length / 2n;
      const middle = first + half;
      if (predicate(middle)) {
        first = middle + 1n;
        length -= half + 1n;
      } else {
        length = half;
      }
    }
    return first;
  }

  function bytesInAccessRange(
    firstElement,
    elementsPerRange,
    strides,
    bitsPerElement,
    selected,
  ) {
    const bounds = accessBounds(
      firstElement,
      elementsPerRange,
      strides,
      bitsPerElement,
    );
    if (!intersectRanges(bounds, selected)) {
      return 0n;
    }
    if (selected[0] <= bounds[0] && selected[1] >= bounds[1]) {
      return accessBytes(
        firstElement,
        elementsPerRange,
        strides,
        bitsPerElement,
      );
    }
    if (strides.length === 0) {
      const intersection = intersectRanges(bounds, selected);
      return intersection ? intersection[1] - intersection[0] : 0n;
    }

    const [stride, ...innerStrides] = strides;
    const strideElements = toBigInt(stride.stride_elements);
    const childBounds = (index) =>
      accessBounds(
        firstElement + index * strideElements,
        elementsPerRange,
        innerStrides,
        bitsPerElement,
      );
    const count = toBigInt(stride.count);
    const first = partitionPoint(
      count,
      (index) => childBounds(index)[1] <= selected[0],
    );
    const end = partitionPoint(
      count,
      (index) => childBounds(index)[0] < selected[1],
    );
    if (first >= end) {
      return 0n;
    }

    let total = bytesInAccessRange(
      firstElement + first * strideElements,
      elementsPerRange,
      innerStrides,
      bitsPerElement,
      selected,
    );
    if (end - first === 1n) {
      return total;
    }
    total += bytesInAccessRange(
      firstElement + (end - 1n) * strideElements,
      elementsPerRange,
      innerStrides,
      bitsPerElement,
      selected,
    );

    const middleCount = end - first - 2n;
    if (middleCount > 0n) {
      total += accessBytes(
        firstElement + (first + 1n) * strideElements,
        elementsPerRange,
        [{ ...stride, count: middleCount }, ...innerStrides],
        bitsPerElement,
      );
    }
    return total;
  }

  function accessBytesInMemory(access, tensorAddress, memoryRange) {
    const base = toBigInt(tensorAddress);
    if (memoryRange[1] <= base) {
      return 0n;
    }
    return bytesInAccessRange(
      toBigInt(access.first_element),
      toBigInt(access.elements_per_range),
      access.strides || [],
      toBigInt(access.bits_per_element),
      [
        memoryRange[0] > base ? memoryRange[0] - base : 0n,
        memoryRange[1] - base,
      ],
    );
  }

  function mergedRanges(ranges) {
    const sorted = ranges
      .filter((range) => range && range[1] > range[0])
      .map((range) => [...range])
      .sort((left, right) =>
        left[0] < right[0] ? -1 : left[0] > right[0] ? 1 : 0,
      );
    const merged = [];
    for (const range of sorted) {
      const previous = merged.at(-1);
      if (previous && range[0] <= previous[1]) {
        if (range[1] > previous[1]) {
          previous[1] = range[1];
        }
      } else {
        merged.push(range);
      }
    }
    return merged;
  }

  function trafficForTransfers(
    transfers,
    tensorAddress,
    selectedLayers = null,
    selectedMemoryRanges = null,
  ) {
    let bytes = 0n;
    let edgeCount = 0;
    const memoryRanges =
      selectedMemoryRanges === null ? null : mergedRanges(selectedMemoryRanges);
    for (const transfer of transfers || []) {
      if (selectedLayers !== null && !selectedLayers.has(transfer.layer)) {
        continue;
      }
      const edgeBytes =
        memoryRanges === null
          ? toBigInt(transfer.access.num_access_bytes)
          : memoryRanges.reduce(
              (total, memoryRange) =>
                total +
                accessBytesInMemory(
                  transfer.access,
                  tensorAddress,
                  memoryRange,
                ),
              0n,
            );
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

  function selectedWindow(
    values,
    limit,
    selected,
    identity = (value) => value,
  ) {
    const window = values.slice(0, limit);
    const selectedValue = values.find((value) => identity(value) === selected);
    if (
      selectedValue &&
      !window.some((value) => identity(value) === selected) &&
      window.length
    ) {
      window[window.length - 1] = selectedValue;
    }
    return window;
  }

  function edgeOrder(left, right) {
    const leftValue = toBigInt(left.value);
    const rightValue = toBigInt(right.value);
    return (
      (leftValue > rightValue ? -1 : leftValue < rightValue ? 1 : 0) ||
      left.source.localeCompare(right.source) ||
      left.target.localeCompare(right.target)
    );
  }

  function strongestEdges(edges, limit, selectedSource = null) {
    const ordered = [...edges].sort(edgeOrder);
    const retained = ordered.slice(0, limit);
    const selectedEdge = ordered.find((edge) => edge.source === selectedSource);
    if (
      selectedEdge &&
      !retained.some((edge) => edge.source === selectedSource) &&
      retained.length
    ) {
      retained[retained.length - 1] = selectedEdge;
      retained.sort(edgeOrder);
    }
    return retained;
  }

  function contextTensorCount(context) {
    return context?.tensors?.length || 0;
  }

  window.GWR_VISUALISATION_VIEW_MODEL = {
    addressRange,
    intersectRanges,
    rangeUnionBytes,
    trafficForTransfers,
    retainedFocus,
    selectedWindow,
    strongestEdges,
    contextTensorCount,
  };
})();
