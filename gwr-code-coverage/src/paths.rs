// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::ffi::OsString;
use std::path::{Component, Path};

#[must_use]
pub fn common_directory_prefix<'a>(paths: impl Iterator<Item = &'a Path>) -> Vec<OsString> {
    let mut common: Option<Vec<OsString>> = None;
    for path in paths {
        let parent_components = path.parent().map(sanitized_components).unwrap_or_default();
        common = Some(match common {
            Some(common) => common
                .into_iter()
                .zip(parent_components)
                .take_while(|(left, right)| left == right)
                .map(|(component, _)| component)
                .collect(),
            None => parent_components,
        });
    }

    common.unwrap_or_default()
}

#[must_use]
pub fn sanitized_components(path: &Path) -> Vec<OsString> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(component) => components.push(component.to_os_string()),
            Component::Prefix(prefix) => components.push(prefix.as_os_str().to_os_string()),
            Component::RootDir | Component::CurDir | Component::ParentDir => {}
        }
    }

    components
}
