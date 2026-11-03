use craft_anything_formats::java_ops::{decode, decode_one};

#[test]
fn invokeinterface_requires_a_count_and_zero_reserved_byte() {
    for code in [[0xb9, 0, 1, 0, 0], [0xb9, 0, 1, 1, 1], [0xb9, 0, 1, 255, 255]] {
        assert!(decode_one(&code, 0).is_none(), "{code:?}");
        assert!(!decode(&code, 16).1);
    }
}

#[test]
fn invokedynamic_requires_both_reserved_bytes_to_be_zero() {
    for code in [[0xba, 0, 1, 1, 0], [0xba, 0, 1, 0, 1], [0xba, 0, 1, 255, 255]] {
        assert!(decode_one(&code, 0).is_none(), "{code:?}");
        assert!(!decode(&code, 16).1);
    }
}

#[test]
fn valid_and_truncated_invocations_keep_their_boundaries() {
    for count in [1, 2, 255] {
        let instruction = decode_one(&[0xb9, 0x12, 0x34, count, 0], 0).unwrap();
        assert_eq!(instruction.cp_index, Some(0x1234));
        assert_eq!(instruction.len, 5);
    }
    let instruction = decode_one(&[0xba, 0x12, 0x34, 0, 0], 0).unwrap();
    assert_eq!(instruction.cp_index, Some(0x1234));
    for code in [[0xb9, 0, 1, 1, 0], [0xba, 0, 1, 0, 0]] {
        for len in 1..5 {
            assert!(decode_one(&code[..len], 0).is_none());
        }
    }
    let (instructions, complete) = decode(&[0xb9, 0, 1, 1, 0, 0xba, 0, 2, 0, 0, 0xb1], 16);
    assert!(complete);
    assert_eq!(instructions.len(), 3);
    assert_eq!(instructions[2].pc, 10);
}
