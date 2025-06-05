// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! The EthernetFrame provides an implementation of a standard Ethernet frame

use std::{fmt::Display, sync::Arc};

use steam_engine::{
    traits::{Routable, SimObject, TotalBytes},
    types::ReqType,
};
use steam_track::{Tag, create_tag, entity::Entity, tag::Tagged};

pub const PREAMBLE_BYTES: usize = 7;
pub const SFD_BYTES: usize = 1;
pub const DEST_MAC_BYTES: usize = 6;
pub const SRC_MAC_BYTES: usize = 6;
pub const PACKET_OVERHEAD_BYTES: usize =
    PREAMBLE_BYTES + SFD_BYTES + DEST_MAC_BYTES + SRC_MAC_BYTES;

pub fn mac_to_u64(mac: &[u8; DEST_MAC_BYTES]) -> u64 {
    ((mac[5] as u64) << (8 * 5))
        | ((mac[4] as u64) << (8 * 4))
        | ((mac[3] as u64) << (8 * 3))
        | ((mac[2] as u64) << (8 * 2))
        | ((mac[1] as u64) << 8)
        | (mac[0] as u64)
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
    pub fn new(create_by: &Arc<Entity>, payload_size_bytes: usize) -> Self {
        Self {
            created_by: create_by.clone(),
            tag: create_tag!(create_by),
            dst_mac: [0; DEST_MAC_BYTES],
            src_mac: [0; DEST_MAC_BYTES],
            payload_size_bytes,
        }
    }

    pub fn set_dest(mut self, dst_mac: [u8; DEST_MAC_BYTES]) -> Self {
        self.dst_mac = dst_mac;
        self
    }

    pub fn set_src(mut self, src_mac: [u8; SRC_MAC_BYTES]) -> Self {
        self.src_mac = src_mac;
        self
    }

    pub fn get_dst(&self) -> u64 {
        mac_to_u64(&self.dst_mac)
    }

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
    fn dest(&self) -> u64 {
        self.get_dst()
    }

    fn req_type(&self) -> ReqType {
        ReqType::Control
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
    fn dest(&self) -> u64 {
        self.as_ref().dest()
    }
    fn req_type(&self) -> ReqType {
        self.as_ref().req_type()
    }
}
