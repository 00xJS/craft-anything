use craft_anything_formats::{bytes::Reader, dex::sleb128};

#[test]
fn rejects_every_overflowing_tenth_byte() {
    for prefix in [0x80, 0xff] {
        for last in 1..0x7f {
            let mut encoded = vec![prefix; 9];
            encoded.push(last);
            assert_eq!(sleb128(&Reader::new(&encoded), 0), None, "last byte {last:#x}");
        }
    }
}

#[test]
fn retains_signed_limits_and_ordinary_values() {
    let mut maximum = vec![0xff; 9];
    maximum.push(0);
    let mut minimum = vec![0x80; 9];
    minimum.push(0x7f);
    assert_eq!(sleb128(&Reader::new(&maximum), 0), Some((i64::MAX, 10)));
    assert_eq!(sleb128(&Reader::new(&minimum), 0), Some((i64::MIN, 10)));
    assert_eq!(sleb128(&Reader::new(&[0x9b, 0xf1, 0x59]), 0), Some((-624485, 3)));
    assert_eq!(sleb128(&Reader::new(&[0xff, 0x7f]), 0), Some((-1, 2)));
    assert_eq!(sleb128(&Reader::new(&[0x80, 0]), 0), Some((0, 2)));
}

#[test]
fn rejects_truncated_values_and_out_of_range_offsets() {
    assert_eq!(sleb128(&Reader::new(&[0x80; 10]), 0), None);
    assert_eq!(sleb128(&Reader::new(&[0x80]), 0), None);
    assert_eq!(sleb128(&Reader::new(&[0]), usize::MAX), None);
}

#[test]
fn overflow_does_not_create_a_catch_all_handler() {
    let mut bytes = vec![0; 0x70];
    bytes[..8].copy_from_slice(b"dex\n035\0");
    bytes[36..40].copy_from_slice(&0x70u32.to_le_bytes());
    bytes[40..44].copy_from_slice(&0x1234_5678u32.to_le_bytes());
    // Empty code item with one try entry pointing to the first encoded handler.
    bytes.extend_from_slice(&[1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    bytes.extend_from_slice(&[0, 0, 0, 0, 0, 0, 1, 0]);
    bytes.push(1);
    bytes.extend_from_slice(&[0x80; 9]);
    bytes.extend_from_slice(&[2, 7]);
    let len = bytes.len() as u32;
    bytes[32..36].copy_from_slice(&len.to_le_bytes());
    let dex = craft_anything_formats::dex::parse(&bytes).unwrap();
    let code = dex.code(0x70).unwrap();
    assert_eq!(code.tries.len(), 1);
    assert!(code.tries[0].handlers.is_empty());
}
