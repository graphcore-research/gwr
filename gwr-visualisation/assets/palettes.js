// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

(() => {
  const App = window.GWR_VISUALISATION_APP;

  const categoricalPalette = [
    "#2a9d8f",
    "#e76f51",
    "#8b5cf6",
    "#6b7280",
    "#84a98c",
    "#d97706",
    "#db2777",
    "#0891b2",
    "#65a30d",
    "#9333ea",
    "#b91c1c",
    "#0f766e",
  ];

  const dataTypeFamilies = [
    ["#546e7a", ["bool", "boolean", "binary"]],
    ["#6d4c41", ["int1", "qint1"]],
    ["#795548", ["int2", "qint2", "ternary"]],
    ["#8d6e63", ["int4", "qint4"]],
    ["#e53935", ["int8", "qint8"]],
    ["#d81b60", ["int16", "qint16"]],
    ["#c2185b", ["int32", "qint32"]],
    ["#880e4f", ["int64", "qint64"]],
    ["#827717", ["uint1", "quint1"]],
    ["#9e9d24", ["uint2", "quint2"]],
    ["#afb42b", ["uint4", "quint4"]],
    ["#7cb342", ["uint8", "quint8"]],
    ["#43a047", ["uint16", "quint16"]],
    ["#00897b", ["uint32", "quint32"]],
    ["#00695c", ["uint64", "quint64"]],
    ["#7e57c2", ["fp4", "float4", "float4_e2m1"]],
    ["#7048b6", ["fp6", "float6", "float6_e2m3", "float6_e3m2"]],
    ["#5e35b1", ["fp8", "float8"]],
    ["#673ab7", ["float8_e4m3", "float8_e4m3fn", "float8_e4m3fnuz"]],
    ["#512da8", ["float8_e5m2", "float8_e5m2fnuz"]],
    ["#3949ab", ["fp16", "float16", "half"]],
    ["#00838f", ["bf16", "bfloat16"]],
    ["#039be5", ["tf32", "tensorfloat32"]],
    ["#1976d2", ["fp32", "float32", "float"]],
    ["#0d47a1", ["fp64", "float64", "double"]],
    ["#f57c00", ["complex32", "complexhalf"]],
    ["#ef6c00", ["complex64", "cfloat"]],
    ["#e65100", ["complex128", "cdouble"]],
    ["#455a64", ["string", "utf8"]],
  ];

  const knownDataTypeColours = new Map(
    dataTypeFamilies.flatMap(([colour, names]) =>
      names.map((name) => [name, colour]),
    ),
  );

  function normaliseDataType(name) {
    return String(name).trim().toLowerCase().replaceAll("-", "_");
  }

  function stableCategoryColours(values) {
    return new Map(
      [...new Set(values)]
        .sort((left, right) =>
          left.localeCompare(right, undefined, { numeric: true }),
        )
        .map((value, index) => [
          value,
          categoricalPalette[index % categoricalPalette.length],
        ]),
    );
  }

  function stableDataTypeColours(values) {
    const colours = new Map();
    const unknown = [];
    for (const value of new Set(values)) {
      const known = knownDataTypeColours.get(normaliseDataType(value));
      if (known) {
        colours.set(value, known);
      } else {
        unknown.push(value);
      }
    }
    for (const [value, colour] of stableCategoryColours(unknown)) {
      colours.set(value, colour);
    }
    return colours;
  }

  Object.assign(App, {
    stableCategoryColours,
    stableDataTypeColours,
  });
})();
