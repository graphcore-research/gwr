// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Helpers for parsing and formatting platform connection endpoint IDs.
//!
//! Endpoint IDs use the same textual forms accepted in platform YAML
//! `connections` entries:
//! - `pe.<name>`
//! - `cache.<name>[.dev|.mem]`
//! - `mem.<name>`
//! - `fabric.<name>@(<column>,<row>)[.<port>]`
//!
//! Fabric endpoints omit `.0` when formatted because port 0 is the default.

use std::fmt;
use std::sync::LazyLock;

use gwr_engine::types::SimError;
use regex::Regex;

/// Identifies one of the cache's external connection ports.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CachePortId {
    /// Device-facing cache port.
    Dev,

    /// Memory-facing cache port.
    Mem,
}

impl CachePortId {
    /// Returns the endpoint suffix used for this cache port.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Mem => "mem",
        }
    }
}

impl fmt::Display for CachePortId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Parsed platform connection endpoint.
///
/// The display form round-trips through [`parse_connection_endpoint_id`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConnectionEndpointId {
    /// Processing element endpoint, formatted as `pe.<name>`.
    Pe {
        /// Processing element name.
        name: String,
    },

    /// Cache endpoint, formatted as `cache.<name>[.dev|.mem]`.
    Cache {
        /// Cache name.
        name: String,

        /// Optional explicit cache port. When omitted, connection builders
        /// infer the side from the other endpoint.
        port: Option<CachePortId>,
    },

    /// Memory endpoint, formatted as `mem.<name>`.
    Mem {
        /// Memory name.
        name: String,
    },

    /// Fabric tile port endpoint, formatted as
    /// `fabric.<name>@(<column>,<row>)[.<port>]`.
    FabricPort {
        /// Fabric name.
        fabric: String,

        /// Fabric column coordinate.
        column: usize,

        /// Fabric row coordinate.
        row: usize,

        /// Ingress/egress port number on the tile. Port 0 is omitted when
        /// formatted.
        port: usize,
    },
}

impl fmt::Display for ConnectionEndpointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pe { name } => write!(f, "pe.{name}"),
            Self::Cache { name, port } => {
                write!(f, "cache.{name}")?;
                if let Some(port) = port {
                    write!(f, ".{port}")?;
                }
                Ok(())
            }
            Self::Mem { name } => write!(f, "mem.{name}"),
            Self::FabricPort {
                fabric,
                column,
                row,
                port,
            } => {
                write!(f, "fabric.{fabric}@({column},{row})")?;
                if *port != 0 {
                    write!(f, ".{port}")?;
                }
                Ok(())
            }
        }
    }
}

/// Parses a platform connection endpoint ID.
///
/// Supported forms are `pe.<name>`, `cache.<name>[.dev|.mem]`,
/// `mem.<name>`, and `fabric.<name>@(<column>,<row>)[.<port>]`. Fabric ports
/// default to port 0 when the suffix is omitted.
pub fn parse_connection_endpoint_id(s: &str) -> Result<ConnectionEndpointId, SimError> {
    if s.starts_with("fabric.") {
        return parse_fabric_port_id(s);
    }

    let mut parts = s.split('.');
    let kind = parts
        .next()
        .ok_or_else(|| SimError(format!("Failed to parse kind in '{s}'")))?;
    let name = parts
        .next()
        .ok_or_else(|| SimError(format!("Failed to parse name in '{s}'")))?;
    let port = parts.next();
    if parts.next().is_some() {
        return Err(SimError(format!("Failed to parse '{s}' - extra tokens")));
    }

    match kind {
        "pe" => {
            if port.is_some() {
                return Err(SimError("Cannot specify a port for PE".to_string()));
            }
            Ok(ConnectionEndpointId::Pe {
                name: name.to_string(),
            })
        }
        "cache" => Ok(ConnectionEndpointId::Cache {
            name: name.to_string(),
            port: port.map(parse_cache_port_id).transpose()?,
        }),
        "mem" => {
            if port.is_some() {
                return Err(SimError("Cannot specify a port for Memory".to_string()));
            }
            Ok(ConnectionEndpointId::Mem {
                name: name.to_string(),
            })
        }
        _ => Err(SimError(format!(
            "Failed to parse '{s}' - unsupported kind"
        ))),
    }
}

fn parse_cache_port_id(s: &str) -> Result<CachePortId, SimError> {
    match s {
        "dev" => Ok(CachePortId::Dev),
        "mem" => Ok(CachePortId::Mem),
        other => Err(SimError(format!("Unsupported Cache port '{other}'"))),
    }
}

fn parse_fabric_port_id(s: &str) -> Result<ConnectionEndpointId, SimError> {
    static FABRIC_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^fabric\.([A-Za-z0-9_]+)@\((\d+),(\d+)\)(?:\.(.*))?$").unwrap()
    });

    let caps = FABRIC_RE
        .captures(s)
        .ok_or_else(|| SimError(format!("Unable to parse Fabric port '{s}'")))?;
    let fabric = caps[1].to_string();
    let column = caps[2].parse().map_err(|e| SimError(format!("{e}")))?;
    let row = caps[3].parse().map_err(|e| SimError(format!("{e}")))?;
    let port = caps
        .get(4)
        .map_or("0", |m| m.as_str())
        .parse()
        .map_err(|e| SimError(format!("{e}")))?;

    Ok(ConnectionEndpointId::FabricPort {
        fabric,
        column,
        row,
        port,
    })
}

/// Formats a processing element endpoint ID.
pub fn pe_endpoint_id(name: impl Into<String>) -> String {
    ConnectionEndpointId::Pe { name: name.into() }.to_string()
}

/// Formats a cache endpoint ID without an explicit cache port.
///
/// Portless cache endpoints let the connection builder infer `dev` or `mem`
/// from the other endpoint.
pub fn cache_endpoint_id(name: impl Into<String>) -> String {
    ConnectionEndpointId::Cache {
        name: name.into(),
        port: None,
    }
    .to_string()
}

/// Formats a memory endpoint ID.
pub fn mem_endpoint_id(name: impl Into<String>) -> String {
    ConnectionEndpointId::Mem { name: name.into() }.to_string()
}

/// Formats a fabric tile port endpoint ID.
///
/// Port 0 is treated as the default and is omitted from the formatted string.
pub fn fabric_port_endpoint_id(
    fabric: impl Into<String>,
    column: usize,
    row: usize,
    port: usize,
) -> String {
    ConnectionEndpointId::FabricPort {
        fabric: fabric.into(),
        column,
        row,
        port,
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        CachePortId, ConnectionEndpointId, fabric_port_endpoint_id, parse_connection_endpoint_id,
    };

    #[test]
    fn parses_and_formats_fabric_port_ids() {
        let endpoint = parse_connection_endpoint_id("fabric.fabric0@(2,3).1")
            .expect("fabric endpoint should parse");

        assert_eq!(
            endpoint,
            ConnectionEndpointId::FabricPort {
                fabric: "fabric0".to_string(),
                column: 2,
                row: 3,
                port: 1,
            }
        );
        assert_eq!(endpoint.to_string(), "fabric.fabric0@(2,3).1");
        assert_eq!(
            fabric_port_endpoint_id("fabric0", 2, 3, 0),
            "fabric.fabric0@(2,3)"
        );
    }

    #[test]
    fn parses_and_formats_cache_port_ids() {
        let endpoint =
            parse_connection_endpoint_id("cache.l1.mem").expect("cache endpoint should parse");

        assert_eq!(
            endpoint,
            ConnectionEndpointId::Cache {
                name: "l1".to_string(),
                port: Some(CachePortId::Mem),
            }
        );
        assert_eq!(endpoint.to_string(), "cache.l1.mem");
    }
}
