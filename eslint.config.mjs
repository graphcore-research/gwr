// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

import js from "@eslint/js";
import globals from "globals";

export default [
  {
    ignores: [
      "target/**",
      "gwr-developer-guide/book/**",
      "gwr-developer-guide/rustdoc_cache/**",
      "gwr-onnx-sys/onnx_src/**",
      "gwr-perfetto-sys/perfetto_src/**",
      "gwr-spotter/frontend/lib/d3.v7.min.js",
      "gwr-spotter/frontend/lib/jquery-3.4.1.js",
    ],
  },
  {
    ...js.configs.recommended,
    languageOptions: {
      globals: { ...globals.browser, ...globals.node },
    },
  },
];
