use craft_anything_formats::axml::StringPool;

fn u32_at(data: &mut [u8], at: usize, value: u32) {
    data[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn pool(utf8: bool, string_data: &[u8], styles: &[u8]) -> Vec<u8> {
    let start = if styles.is_empty() { 32 } else { 36 };
    let mut data = vec![0; start];
    data[..2].copy_from_slice(&1u16.to_le_bytes());
    data[2..4].copy_from_slice(&28u16.to_le_bytes());
    u32_at(&mut data, 8, 1);
    u32_at(&mut data, 12, u32::from(!styles.is_empty()));
    u32_at(&mut data, 16, if utf8 { 1 << 8 } else { 0 });
    u32_at(&mut data, 20, start as u32);
    if !styles.is_empty() {
        u32_at(&mut data, 24, (start + string_data.len()) as u32);
    }
    data.extend(string_data);
    data.extend(styles);
    data.resize(data.len().next_multiple_of(4), 0);
    let size = data.len() as u32;
    u32_at(&mut data, 4, size);
    data
}

fn valid_pool() -> Vec<u8> {
    pool(true, b"\x04\x04kept\0", &[])
}

#[test]
fn fixed_fields_must_be_inside_the_declared_header_and_chunk() {
    for header in [8u16, 24] {
        let mut data = valid_pool();
        data[2..4].copy_from_slice(&header.to_le_bytes());
        if header == 8 {
            u32_at(&mut data, 4, 8);
        }
        assert!(StringPool::parse(&data, 0).is_err());
    }
}

#[test]
fn every_declared_offset_must_fit_inside_the_chunk() {
    let mut data = valid_pool();
    u32_at(&mut data, 8, 2);
    u32_at(&mut data, 4, 32);
    assert!(StringPool::parse(&data, 0).is_err());
}

#[test]
fn string_data_cannot_overlap_the_header_or_offset_tables() {
    for start in [0, 16, 28] {
        let mut data = valid_pool();
        u32_at(&mut data, 20, start);
        assert!(StringPool::parse(&data, 0).is_err());
    }
    let mut data = valid_pool();
    u32_at(&mut data, 12, 1);
    assert!(StringPool::parse(&data, 0).is_err(), "style offsets also take space before string data");
}

#[test]
fn utf8_strings_cannot_borrow_bytes_from_style_data() {
    let data = pool(true, b"\x04\x04ok", b"xy\0\0\0\0\0\0\0\0\0\0");
    let parsed = StringPool::parse(&data, 0).unwrap();
    assert_eq!(parsed.get(0), Some(""));
}

#[test]
fn utf16_strings_cannot_borrow_bytes_from_style_data() {
    let data = pool(false, b"\x02\0o\0", b"k\0\0\0\0\0\0\0\0\0\0\0");
    let parsed = StringPool::parse(&data, 0).unwrap();
    assert_eq!(parsed.get(0), Some(""));
}

#[test]
fn a_pool_at_a_nonzero_offset_uses_only_its_own_bytes() {
    let mut data = vec![0x7f; 13];
    data.extend(valid_pool());
    data.extend([0x5a; 64]);
    assert_eq!(StringPool::parse(&data, 13).unwrap().get(0), Some("kept"));
}

#[test]
fn empty_pools_with_no_string_data_remain_valid() {
    let mut data = vec![0; 28];
    data[..2].copy_from_slice(&1u16.to_le_bytes());
    data[2..4].copy_from_slice(&28u16.to_le_bytes());
    u32_at(&mut data, 4, 28);
    assert!(StringPool::parse(&data, 0).unwrap().strings.is_empty());
}
