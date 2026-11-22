use tritdb_radix::{pack, unpack, Radix, RadixError, Trit};

fn trits_for_len(len: usize) -> Vec<Trit> {
    (0..len)
        .map(|index| Trit::from_u8((index % 3) as u8).expect("valid trit"))
        .collect()
}

#[test]
fn roundtrip_all_lengths_all_radices() {
    for radix in Radix::ALL {
        for len in 0..=2048 {
            let input = trits_for_len(len);
            let packed = pack(radix, &input).expect("pack");
            assert_eq!(packed.len(), radix.packed_len(len));
            let decoded = unpack(radix, &packed, len).expect("unpack");
            assert_eq!(decoded, input, "radix={radix:?} len={len}");
        }
    }
}

#[test]
fn packed_len_matches_bytes_for_empty_and_partial() {
    for radix in Radix::ALL {
        for len in 0..=32 {
            let input = trits_for_len(len);
            let packed = pack(radix, &input).expect("pack");
            assert_eq!(packed.len(), radix.packed_len(len));
            assert!(packed.len() <= len + 1);
        }
    }
}

#[test]
fn deterministic_pack_is_stable() {
    let input = trits_for_len(1234);
    for radix in Radix::ALL {
        let a = pack(radix, &input).expect("pack");
        let b = pack(radix, &input).expect("pack");
        assert_eq!(a, b);
    }
}

#[test]
fn validate_packed_accepts_all_roundtrips() {
    for radix in Radix::ALL {
        for len in 0..=2048 {
            let input = trits_for_len(len);
            let packed = pack(radix, &input).expect("pack");
            radix
                .validate_packed(&packed, len)
                .expect("canonical packed bytes");
        }
    }
}

#[test]
fn rejects_trailing_garbage_length() {
    let input = trits_for_len(5);
    for radix in Radix::ALL {
        let mut packed = pack(radix, &input).expect("pack");
        packed.push(0);
        let error = unpack(radix, &packed, input.len()).expect_err("must reject");
        match error {
            RadixError::LengthMismatch { expected, actual } => {
                assert_eq!(expected, radix.packed_len(input.len()));
                assert_eq!(actual, expected + 1);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}

#[test]
fn non_canonical_padding_is_rejected_for_all_radices() {
    // One trit, canonical byte is `T2` = 2. Each radix has unused slots.
    assert!(matches!(
        unpack(Radix::R3, &[5], 1),
        Err(RadixError::NonCanonicalPadding { .. })
    ));
    assert!(matches!(
        unpack(Radix::R4, &[0b0000_1000], 1),
        Err(RadixError::NonCanonicalPadding { .. })
    ));
    assert!(matches!(
        unpack(Radix::R16, &[0x12], 1),
        Err(RadixError::NonCanonicalPadding { .. })
    ));
}
