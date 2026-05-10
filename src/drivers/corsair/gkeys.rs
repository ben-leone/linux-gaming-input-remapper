/// Report ID used by the K95 RGB (PID 0x1B11) G-key HID reports.
pub const K95_REPORT_ID: u8 = 0x03;

/// Total number of G-keys on the K95 RGB.
pub const GKEY_COUNT: u32 = 18;

/// Byte layout (K95 RGB PID 0x1B11, report ID 0x03):
///   Byte 16, bits 0–7 → G1–G8
///   Byte 17, bits 0–1 → G9–G10  (bits 2–7 unused)
///   Byte 18, bits 0–7 → G11–G18
///
/// Decode a raw hidraw report into an 18-bit G-key pressed bitmask.
/// Bit N-1 set → G-key N is held (bit 0 = G1, bit 17 = G18).
/// Returns None if this report is not a G-key report.
pub fn decode(data: &[u8]) -> Option<u32> {
    if data.len() < 19 || data[0] != K95_REPORT_ID {
        return None;
    }
    let b16 = data[16] as u32;            // G1–G8  → bits 0–7
    let b17 = (data[17] & 0x03) as u32;  // G9–G10 → bits 8–9
    let b18 = data[18] as u32;            // G11–G18 → bits 10–17
    Some(b16 | (b17 << 8) | (b18 << 10))
}

/// Format a G-key bitmask as a human-readable string ("G1", "G3+G5", "—").
pub fn names(mask: u32) -> String {
    if mask == 0 {
        return "—".to_string();
    }
    (0..GKEY_COUNT)
        .filter(|&i| mask & (1 << i) != 0)
        .map(|i| format!("G{}", i + 1))
        .collect::<Vec<_>>()
        .join("+")
}

/// Linux key codes for G1–G18 (KEY_MACRO1–KEY_MACRO18, starting at 0x290).
pub fn keycode(gkey_index: u32) -> u16 {
    0x290 + gkey_index as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_byte16_keys() {
        let mut data = [0u8; 64];
        data[0] = K95_REPORT_ID;

        data[16] = 0x01; assert_eq!(decode(&data), Some(1 << 0));  // G1
        data[16] = 0x02; assert_eq!(decode(&data), Some(1 << 1));  // G2
        data[16] = 0x40; assert_eq!(decode(&data), Some(1 << 6));  // G7
        data[16] = 0x80; assert_eq!(decode(&data), Some(1 << 7));  // G8
    }

    #[test]
    fn test_byte17_keys() {
        let mut data = [0u8; 64];
        data[0] = K95_REPORT_ID;

        data[17] = 0x01; assert_eq!(decode(&data), Some(1 << 8));  // G9
        data[17] = 0x02; assert_eq!(decode(&data), Some(1 << 9));  // G10
        // bits 2–7 of byte 17 are unused
        data[17] = 0xFC; assert_eq!(decode(&data), Some(0));
    }

    #[test]
    fn test_byte18_keys() {
        let mut data = [0u8; 64];
        data[0] = K95_REPORT_ID;

        data[18] = 0x01; assert_eq!(decode(&data), Some(1 << 10)); // G11
        data[18] = 0x02; assert_eq!(decode(&data), Some(1 << 11)); // G12
        data[18] = 0x40; assert_eq!(decode(&data), Some(1 << 16)); // G17
        data[18] = 0x80; assert_eq!(decode(&data), Some(1 << 17)); // G18
    }

    #[test]
    fn test_simultaneous() {
        let mut data = [0u8; 64];
        data[0] = K95_REPORT_ID;
        // G7 + G8 (same byte)
        data[16] = 0xC0;
        assert_eq!(decode(&data), Some((1 << 6) | (1 << 7)));
        // G8 + G9 across byte16/byte17 boundary
        data[16] = 0x80; data[17] = 0x01;
        assert_eq!(decode(&data), Some((1 << 7) | (1 << 8)));
        // G10 + G11 across byte17/byte18 boundary
        data[16] = 0x00; data[17] = 0x02; data[18] = 0x01;
        assert_eq!(decode(&data), Some((1 << 9) | (1 << 10)));
    }

    #[test]
    fn test_names() {
        assert_eq!(names(0), "—");
        assert_eq!(names(1 << 0),  "G1");
        assert_eq!(names(1 << 6),  "G7");
        assert_eq!(names(1 << 7),  "G8");
        assert_eq!(names(1 << 8),  "G9");
        assert_eq!(names(1 << 9),  "G10");
        assert_eq!(names(1 << 10), "G11");
        assert_eq!(names(1 << 17), "G18");
        assert_eq!(names((1 << 6) | (1 << 7)), "G7+G8");
    }

    #[test]
    fn test_wrong_report_id() {
        let mut data = [0u8; 64];
        data[0] = 0x01;
        data[16] = 0xFF;
        data[17] = 0xFF;
        assert_eq!(decode(&data), None);
    }
}
