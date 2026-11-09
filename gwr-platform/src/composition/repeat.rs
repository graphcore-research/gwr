// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Bounded, deterministic endpoint repetition; this is not a template language.
use super::{
    BTreeMap, DEFAULT_MATERIALIZATION_LIMIT, Deserialize, Serialize, SimError, err, identifier,
};

/// Integer half-open loops over endpoint pairs. Only index expressions
/// substitute.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionRepeat {
    #[serde(rename = "for")]
    pub ranges: BTreeMap<String, [u32; 2]>,
    pub connect: [String; 2],
}
impl ConnectionRepeat {
    pub fn count(&self) -> Result<u64, SimError> {
        if self.ranges.is_empty() || self.ranges.len() > 16 {
            return Err(err("Connection repeat requires a loop variable"));
        }
        let mut total = 1u64;
        for (var, [start, end]) in &self.ranges {
            if !identifier(var) || var.as_bytes()[0].is_ascii_digit() || start > end {
                return Err(err("Invalid loop variable/range"));
            }
            total = total
                .checked_mul(u64::from(end - start))
                .ok_or_else(|| err("Repeat size overflow"))?;
        }
        if total > DEFAULT_MATERIALIZATION_LIMIT as u64 {
            return Err(err("Connection repeat exceeds expansion limit"));
        }
        Ok(total)
    }
    pub fn expand(&self) -> Result<Vec<Vec<String>>, SimError> {
        if self.count()? == 0 {
            return Ok(vec![]);
        }
        let mut out = vec![];
        self.walk(
            &self.ranges.iter().collect::<Vec<_>>(),
            &mut BTreeMap::new(),
            &mut out,
        )?;
        Ok(out)
    }
    fn walk(
        &self,
        ranges: &[(&String, &[u32; 2])],
        vars: &mut BTreeMap<String, i128>,
        out: &mut Vec<Vec<String>>,
    ) -> Result<(), SimError> {
        if let Some(((name, [start, end]), rest)) = ranges.split_first() {
            for i in *start..*end {
                vars.insert((*name).clone(), i128::from(i));
                self.walk(rest, vars, out)?;
            }
        } else {
            out.push(
                self.connect
                    .iter()
                    .map(|s| substitute(s, vars))
                    .collect::<Result<Vec<_>, _>>()?,
            );
        }
        Ok(())
    }
}
fn substitute(s: &str, vars: &BTreeMap<String, i128>) -> Result<String, SimError> {
    let mut out = String::new();
    let mut rest = s;
    while let Some((before, after)) = rest.split_once("${") {
        out.push_str(before);
        let (expression, after) = after
            .split_once('}')
            .ok_or_else(|| err("Unclosed index expression"))?;
        if expression.len() > 512 {
            return Err(err("Index expression too long"));
        }
        let mut parser = Expression {
            bytes: expression.as_bytes(),
            pos: 0,
            vars,
            depth: 0,
        };
        let value = parser.sum()?;
        parser.space();
        if parser.pos != parser.bytes.len() || value < 0 || value > i128::from(u64::MAX) {
            return Err(err(format!("Invalid index expression {expression}")));
        }
        out.push_str(&value.to_string());
        rest = after;
    }
    out.push_str(rest);
    Ok(out)
}
struct Expression<'a> {
    bytes: &'a [u8],
    pos: usize,
    vars: &'a BTreeMap<String, i128>,
    depth: usize,
}
impl Expression<'_> {
    fn space(&mut self) {
        while self
            .bytes
            .get(self.pos)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.pos += 1;
        }
    }
    fn sum(&mut self) -> Result<i128, SimError> {
        let mut n = self.product()?;
        loop {
            self.space();
            match self.bytes.get(self.pos) {
                Some(b'+') => {
                    self.pos += 1;
                    n = n
                        .checked_add(self.product()?)
                        .ok_or_else(|| err("Index arithmetic overflow"))?;
                }
                Some(b'-') => {
                    self.pos += 1;
                    n = n
                        .checked_sub(self.product()?)
                        .ok_or_else(|| err("Index arithmetic overflow"))?;
                }
                _ => return Ok(n),
            }
        }
    }
    fn product(&mut self) -> Result<i128, SimError> {
        let mut n = self.atom()?;
        loop {
            self.space();
            if self.bytes.get(self.pos) != Some(&b'*') {
                return Ok(n);
            }
            self.pos += 1;
            n = n
                .checked_mul(self.atom()?)
                .ok_or_else(|| err("Index arithmetic overflow"))?;
        }
    }
    fn atom(&mut self) -> Result<i128, SimError> {
        self.space();
        if self.bytes.get(self.pos) == Some(&b'(') {
            self.depth += 1;
            if self.depth > 32 {
                return Err(err("Index expression nesting limit"));
            }
            self.pos += 1;
            let n = self.sum()?;
            self.space();
            if self.bytes.get(self.pos) != Some(&b')') {
                return Err(err("Missing closing parenthesis"));
            }
            self.pos += 1;
            self.depth -= 1;
            return Ok(n);
        }
        let start = self.pos;
        while self
            .bytes
            .get(self.pos)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
        {
            self.pos += 1;
        }
        let token = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap();
        if token.bytes().all(|c| c.is_ascii_digit()) && !token.is_empty() {
            token.parse().map_err(|_| err("Index integer overflow"))
        } else {
            self.vars
                .get(token)
                .copied()
                .ok_or_else(|| err(format!("Unknown index variable '{token}'")))
        }
    }
}
