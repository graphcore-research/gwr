// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! The EthernetFrame provides an implementation of a standard Ethernet frame

use std::fmt::Display;
use std::sync::Arc;

use steam_engine::traits::{Routable, SimObject, TotalBytes};
use steam_engine::types::{AccessType, SimError};
use steam_track::entity::Entity;
use steam_track::tag::Tagged;
use steam_track::{Tag, create, create_tag};

pub const PREAMBLE_BYTES: usize = 7;
pub const SFD_BYTES: usize = 1;
pub const DEST_MAC_BYTES: usize = 6;
pub const SRC_MAC_BYTES: usize = 6;
pub const PACKET_OVERHEAD_BYTES: usize =
    PREAMBLE_BYTES + SFD_BYTES + DEST_MAC_BYTES + SRC_MAC_BYTES;

#[must_use]
pub fn mac_to_u64(mac: &[u8; DEST_MAC_BYTES]) -> u64 {
    ((mac[5] as u64) << (8 * 5))
        | ((mac[4] as u64) << (8 * 4))
        | ((mac[3] as u64) << (8 * 3))
        | ((mac[2] as u64) << (8 * 2))
        | ((mac[1] as u64) << 8)
        | (mac[0] as u64)
}

#[must_use]
pub fn u64_to_mac(value: u64) -> [u8; DEST_MAC_BYTES] {
    let mut mac = [0_u8; DEST_MAC_BYTES];
    mac[0] = (value & 0xff) as u8;
    mac[1] = ((value >> 8) & 0xff) as u8;
    mac[2] = ((value >> (8 * 2)) & 0xff) as u8;
    mac[3] = ((value >> (8 * 3)) & 0xff) as u8;
    mac[4] = ((value >> (8 * 4)) & 0xff) as u8;
    mac[5] = ((value >> (8 * 5)) & 0xff) as u8;
    mac
}

#[derive(Clone, Debug)]
pub struct EthernetFrame {
    created_by: Arc<Entity>,
    tag: Tag,

    // We don't include the Preamble / SFD bytes in the frame contents
    dst_mac: [u8; DEST_MAC_BYTES],
    src_mac: [u8; SRC_MAC_BYTES],

    // Currently we don't store any actual frame contents
    payload_size_bytes: usize,
}

impl EthernetFrame {
    #[must_use]
    pub fn new(created_by: &Arc<Entity>, payload_size_bytes: usize) -> Self {
        let frame = Self {
            created_by: created_by.clone(),
            tag: create_tag!(created_by),
            dst_mac: [0; DEST_MAC_BYTES],
            src_mac: [0; DEST_MAC_BYTES],
            payload_size_bytes,
        };
        // Having just created the frame the req_type must be valid
        create!(created_by ; frame, frame.total_bytes(), frame.req_type().unwrap() as i8);
        frame
    }

    #[must_use]
    pub fn set_dest(mut self, dst_mac: [u8; DEST_MAC_BYTES]) -> Self {
        self.dst_mac = dst_mac;
        self
    }

    #[must_use]
    pub fn set_src(mut self, src_mac: [u8; SRC_MAC_BYTES]) -> Self {
        self.src_mac = src_mac;
        self
    }

    #[must_use]
    pub fn get_dst(&self) -> u64 {
        mac_to_u64(&self.dst_mac)
    }

    #[must_use]
    pub fn get_src(&self) -> u64 {
        mac_to_u64(&self.src_mac)
    }
}

impl SimObject for EthernetFrame {}

impl Display for EthernetFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{}: {:?} -> {:?} ({} bytes)",
            self.created_by, self.src_mac, self.dst_mac, self.payload_size_bytes
        )
    }
}

impl TotalBytes for EthernetFrame {
    fn total_bytes(&self) -> usize {
        self.payload_size_bytes + PREAMBLE_BYTES + SFD_BYTES + DEST_MAC_BYTES + SRC_MAC_BYTES
    }
}

impl Tagged for EthernetFrame {
    fn tag(&self) -> Tag {
        self.tag
    }
}

impl Routable for EthernetFrame {
    fn dest(&self) -> Result<u64, SimError> {
        Ok(self.get_dst())
    }

    fn req_type(&self) -> Result<AccessType, SimError> {
        Ok(AccessType::Control)
    }
}

/// Allow Box of any SimObject type to be used
impl SimObject for Box<EthernetFrame> {}

impl TotalBytes for Box<EthernetFrame> {
    fn total_bytes(&self) -> usize {
        self.as_ref().total_bytes()
    }
}

impl Tagged for Box<EthernetFrame> {
    fn tag(&self) -> steam_track::Tag {
        self.as_ref().tag()
    }
}

impl Routable for Box<EthernetFrame> {
    fn dest(&self) -> Result<u64, SimError> {
        self.as_ref().dest()
    }
    fn req_type(&self) -> Result<AccessType, SimError> {
        self.as_ref().req_type()
    }
}
