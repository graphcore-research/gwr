// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::Error;
use crate::coverage::file::CoverageSummary;
use crate::coverage::{CoverageFile, strip_prefix};

#[derive(Debug, Clone, Deserialize)]
pub struct CoverageReport {
    data: Vec<CoverageData>,
}

impl CoverageReport {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, Error> {
        let report = fs::read_to_string(path)?;
        Self::from_json(&report)
    }

    pub fn from_json(report: &str) -> Result<Self, Error> {
        let report: Self = serde_json::from_str(report)?;
        if report.data.is_empty() {
            return Err(Error::EmptyReport);
        }
        Ok(report)
    }

    pub(crate) fn totals(&self) -> &CoverageSummary {
        &self.data[0].totals
    }

    pub(crate) fn files(&self, prefix: &str) -> BTreeMap<String, &CoverageSummary> {
        self.data[0]
            .files
            .iter()
            .map(|file| (strip_prefix(&file.filename, prefix), &file.summary))
            .collect()
    }

    pub(crate) fn coverage_files(&self, prefix: &str) -> BTreeMap<String, &CoverageFile> {
        self.data[0]
            .files
            .iter()
            .map(|file| (strip_prefix(&file.filename, prefix), file))
            .collect()
    }

    pub fn filenames(&self) -> impl Iterator<Item = &str> {
        self.data[0].files.iter().map(|file| file.filename.as_str())
    }

    pub fn map_filenames(mut self, mut map_filename: impl FnMut(&str) -> Option<String>) -> Self {
        for file in &mut self.data[0].files {
            if let Some(filename) = map_filename(&file.filename) {
                file.filename = filename;
            }
        }
        self
    }

    pub(crate) fn has_line_details(&self) -> bool {
        self.data[0]
            .files
            .iter()
            .any(CoverageFile::has_line_details)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CoverageData {
    files: Vec<CoverageFile>,
    totals: CoverageSummary,
}
