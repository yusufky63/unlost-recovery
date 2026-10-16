//! CRC-32C (Castagnoli), the checksum VHDX uses for its headers and region tables.

const POLY: u32 = 0x82F6_3B78;

const TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { (c >> 1) ^ POLY } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

pub fn checksum(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc = TABLE[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

#[cfg(test)]
mod tests {
    #[test]
    fn known_vectors() {
        // RFC 3720, B.4.
        assert_eq!(super::checksum(&[0u8; 32]), 0x8A91_36AA);
        assert_eq!(super::checksum(&[0xFFu8; 32]), 0x62A8_AB43);
        assert_eq!(super::checksum(b"123456789"), 0xE306_9283);
    }
}
