<!-- Copyright (c) 2025 Graphcore Ltd. All rights reserved. -->

# API Documentation

<!-- cmdrun bash -x -c "cargo clean --doc --target-dir ../../rustdoc_cache" > ../../rustdoc_build.log 2>&1 -->
<!-- cmdrun bash -x -c "STEAM_DOCS_ONLY=1 cargo doc-steam --target-dir ../../rustdoc_cache" >> ../../rustdoc_build.log 2>&1 -->
<!-- cmdrun bash -x -c "rm -rfv ../../book/rustdoc" >> ../../rustdoc_build.log 2>&1 -->
<!-- cmdrun bash -x -c "mkdir -pv ../../book/rustdoc" >> ../../rustdoc_build.log 2>&1 -->
<!-- cmdrun bash -x -c "cp -rv ../../rustdoc_cache/doc ../../book/rustdoc" >> ../../rustdoc_build.log 2>&1 -->

<iframe src="../../rustdoc/steam_engine/index.html" width="100%" height="600" style="border:1px solid black;">
</iframe>

<div class="warning">

Please note the `rustdoc` output cannot be reached when this page is accessed
via `mdbook serve`. Should you wish to view the API documentation run
`mdbook build` or `mdbook watch` instead.

The documentation will then be displayed in the above frame, as well as being
accessable
<a href="../../rustdoc/steam_engine/index.html" target="_blank">externally</a>
to this developer guide.

</div>
