// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! A utility to pre-process text before emitting as AsciiDoctor.

use regex::Regex;

pub struct AsciiDoctorPreProcessor {
    link_re: Regex,
}

impl AsciiDoctorPreProcessor {
    #[must_use]
    pub fn new() -> Self {
        Self {
            link_re: Regex::new(r"\[`(?<text>[^`]+)`\]\((?<link>[^\)]+)\)").unwrap(),
        }
    }

    /// Preprocess the documentation strings for emitting as asciidoctor
    #[must_use]
    pub fn preprocess_doc(&self, input: &str, depth: usize) -> String {
        let mut output = Vec::new();
        for line in input.lines() {
            let line = Self::strip_leading_space(line.to_string());
            let line = Self::reindent_headings(line, depth);
            let line = self.process_links(&line);
            output.push(line);
        }
        output.join("\n")
    }

    /// Clean up leading space from lines which will have come from doc strings
    ///
    /// Most documentation strings will be written with a space after the
    /// leading comment indicator. However, this breaks the MD if it is
    /// maintained, so it needs to be stripped from every line. The
    /// assumption is that one space is removed from any line that has one
    /// or more spaces.
    fn strip_leading_space(line: String) -> String {
        if line.starts_with(' ') {
            let mut line = line.clone();
            line.remove(0);
            line
        } else {
            line
        }
    }

    /// Update the depth of existing AsciiDoctor headings.
    ///
    /// Blocks of documentation can be included at different locations within
    /// documentation. As a result, their headings need to be set to the right
    /// depth for where they are being inserted.
    ///
    /// For example, the documentation within a file should be written as
    /// top-level:
    ///
    /// ```text
    /// = Section
    ///
    /// With some text.
    ///
    /// == Sub Section
    ///
    /// With more text
    /// ```
    ///
    /// Might need to be replaced with:
    /// ```text
    /// === Section
    ///
    /// With some text.
    ///
    /// ==== Sub Section
    ///
    /// With more text
    /// ```
    fn reindent_headings(line: String, depth: usize) -> String {
        let heading_depth = line.chars().take_while(|ch| *ch == '=').count();

        if heading_depth > 0 {
            // Ensure the next character is a space
            let next_char = line.chars().nth(heading_depth);
            if let Some(ch) = next_char {
                if ch == ' ' {
                    let (_old_heading, rest) = line.split_at(heading_depth);
                    let new_depth = heading_depth + depth - 2;
                    return format!("{}{}", "=".repeat(new_depth), rest).to_owned();
                }
            }
        }
        line
    }

    #[allow(rustdoc::broken_intra_doc_links)]
    /// Process the markdown links and replace them with AsciiDoctor ones.
    ///
    /// Markdown links should be of the form: [`Visible Text`](link)
    /// Which are replace with adoc versions: <<link, Visible Text>>
    fn process_links(&self, line: &str) -> String {
        let mut line = line.to_owned();
        while let Some(e) = self.link_re.captures(&line) {
            let md_link = e.get(0).unwrap().as_str();
            let text = e.name("text").unwrap().as_str();
            let link = e.name("link").unwrap().as_str();
            let adoc_link = format!("<<{link},{text}>>").to_owned();
            line = line.replace(md_link, adoc_link.as_str());
        }
        line
    }
}

impl Default for AsciiDoctorPreProcessor {
    fn default() -> Self {
        Self::new()
    }
}
