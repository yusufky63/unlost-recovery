//! Tags for content locations the broker hands out.
//!
//! The broker keeps nothing per file: each location it streams to the UI carries an
//! HMAC-SHA256 tag made with a random key that exists only in the broker's memory, one key per
//! scan. A read request must bring the location back with its tag, so the UI (or anything that
//! got into it) can only make the elevated broker read clusters the broker itself found for
//! that record in that scan, never arbitrary sectors.

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::{Result, system};

/// Tag bytes kept: 128 bits.
pub const TAG_BYTES: usize = 16;

pub struct Sealer {
    key: [u8; 32],
    scan: u32,
}

impl Sealer {
    pub fn new(scan: u32) -> Result<Self> {
        let mut key = [0u8; 32];
        system::random_bytes(&mut key)?;
        Ok(Sealer { key, scan })
    }

    fn mac(&self, record: u64, location: &[u8]) -> Hmac<Sha256> {
        // Any key length is valid for HMAC, so this cannot fail.
        let mut m = <Hmac<Sha256> as Mac>::new_from_slice(&self.key).expect("HMAC key");
        m.update(b"unlost content location\0");
        m.update(&self.scan.to_le_bytes());
        m.update(&record.to_le_bytes());
        m.update(&(location.len() as u64).to_le_bytes());
        m.update(location);
        m
    }

    pub fn tag(&self, record: u64, location: &[u8]) -> Vec<u8> {
        self.mac(record, location).finalize().into_bytes()[..TAG_BYTES].to_vec()
    }

    /// Constant-time check of a tag handed back with a read request.
    pub fn verify(&self, record: u64, location: &[u8], tag: &[u8]) -> bool {
        tag.len() == TAG_BYTES
            && self
                .mac(record, location)
                .verify_truncated_left(tag)
                .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_bind_key_scan_record_and_location() {
        let s = Sealer::new(7).unwrap();
        let t = s.tag(40, b"loc");
        assert!(s.verify(40, b"loc", &t));
        assert!(!s.verify(41, b"loc", &t));
        assert!(!s.verify(40, b"lod", &t));
        assert!(!s.verify(40, b"loc", &t[..8]));
        assert!(!s.verify(40, b"loc", &[0u8; TAG_BYTES]));
        // Another scan, or the same scan id after a restart, has another key.
        assert!(!Sealer::new(7).unwrap().verify(40, b"loc", &t));
    }
}
