//! Exhaustive and randomized conformance tests for the radix layer.

use tritdb_radix::{pack, pack_into, unpack, unpack_into, Radix, RadixError, Trit};
use tritdb_testkit::Rng;

fn trits_from(values: &[u8]) -> Vec<Trit> {
    values
        .iter()
        .map(|value| Trit::from_u8(*value).expect("trit in 0..=2"))
        .collect()
}

#[test]
fn exhaustive_r3_codebook_is_a_bijection() {
    let mut seen = [false; 243];
    for byte in 0..=242u8 {
        let trits = unpack(Radix::R3, &[byte], 5).expect("decode valid r3 byte");
        assert_eq!(trits.len(), 5);
        let reparsed = pack(Radix::R3, &trits).expect("re-encode");
        assert_eq!(reparsed, vec![byte]);
        assert!(!seen[byte as usize], "r3 byte mapped twice: {byte}");
        seen[byte as usize] = true;
    }
    assert!(
        seen.iter().all(|flag| *flag),
        "not all 243 bytes were covered"
    );

    for byte in 243..=255u8 {
        let error = unpack(Radix::R3, &[byte], 5).expect_err("invalid r3 byte");
        assert!(
            matches!(error, RadixError::InvalidPackedByte { radix: Radix::R3, byte: found } if found == byte),
            "unexpected error for {byte}: {error:?}"
        );
    }
}

#[test]
fn exhaustive_r4_slots() {
    let mut valid = 0usize;
    for byte in 0..=255u8 {
        let slots = [
            byte & 0b11,
            (byte >> 2) & 0b11,
            (byte >> 4) & 0b11,
            (byte >> 6) & 0b11,
        ];
        if slots.iter().all(|slot| *slot <= 2) {
            valid += 1;
            let trits = unpack(Radix::R4, &[byte], 4).expect("valid r4 byte");
            assert_eq!(trits.len(), 4);
            assert_eq!(pack(Radix::R4, &trits).expect("re-encode"), vec![byte]);
        } else {
            let error = unpack(Radix::R4, &[byte], 4).expect_err("invalid r4 byte");
            assert!(
                matches!(
                    error,
                    RadixError::InvalidPackedByte {
                        radix: Radix::R4,
                        ..
                    }
                ),
                "unexpected error for {byte}: {error:?}"
            );
        }
    }
    assert_eq!(valid, 3usize.pow(4), "r4 valid byte count");
}

#[test]
fn exhaustive_r16_nibbles() {
    let mut valid = 0usize;
    for byte in 0..=255u8 {
        let low = byte & 0x0f;
        let high = byte >> 4;
        if low <= 2 && high <= 2 {
            valid += 1;
            let trits = unpack(Radix::R16, &[byte], 2).expect("valid r16 byte");
            assert_eq!(trits, trits_from(&[low, high]));
            assert_eq!(pack(Radix::R16, &trits).expect("re-encode"), vec![byte]);
        } else {
            let error = unpack(Radix::R16, &[byte], 2).expect_err("invalid r16 byte");
            assert!(
                matches!(
                    error,
                    RadixError::InvalidPackedByte {
                        radix: Radix::R16,
                        ..
                    }
                ),
                "unexpected error for {byte}: {error:?}"
            );
        }
    }
    assert_eq!(valid, 3usize.pow(2), "r16 valid byte count");
}

#[test]
fn every_length_roundtrips_all_radices_and_reuses_buffers() {
    for radix in Radix::ALL {
        for count in 0..=1024usize {
            let mut rng = Rng::new(0x51E0_0000 + count as u64);
            let values: Vec<u8> = (0..count).map(|_| rng.trit()).collect();
            let trits = trits_from(&values);

            let packed = pack(radix, &trits).expect("pack");
            assert_eq!(packed.len(), radix.packed_len(count));
            radix
                .validate_packed(&packed, count)
                .expect("canonical pack");

            assert_eq!(
                unpack(radix, &packed, count).expect("unpack"),
                trits,
                "roundtrip mismatch for {} count {}",
                radix.name(),
                count
            );

            let mut pack_buffer = vec![0xff; packed.len() + 7];
            pack_into(radix, &trits, &mut pack_buffer).expect("pack_into");
            assert_eq!(pack_buffer, packed, "pack_into must clear and replace");

            let mut unpack_buffer = vec![Trit::T2; count + 7];
            unpack_into(radix, &packed, count, &mut unpack_buffer).expect("unpack_into");
            assert_eq!(unpack_buffer, trits, "unpack_into must clear and replace");
        }
    }
}

#[test]
fn cross_radix_repack_preserves_trits() {
    for seed in 0..16u64 {
        let mut rng = Rng::new(0xC0DE_0000 + seed);
        let count = (rng.below(2048) + 1) as usize;
        let values: Vec<u8> = (0..count).map(|_| rng.trit()).collect();
        let trits = trits_from(&values);

        let mut lengths = Vec::new();
        for radix in Radix::ALL {
            let packed = pack(radix, &trits).expect("pack");
            let decoded = unpack(radix, &packed, count).expect("unpack");
            assert_eq!(decoded, trits, "radix {} changed trits", radix.name());
            lengths.push(packed.len());
        }
        assert!(lengths[0] <= lengths[1] && lengths[1] <= lengths[2]);
    }
}

#[test]
fn non_canonical_padding_is_rejected_for_all_partial_lengths() {
    for radix in Radix::ALL {
        let per_byte = radix.trits_per_byte();
        for rem in 1..per_byte {
            let values: Vec<u8> = (0..rem).map(|index| (index % 3) as u8).collect();
            let trits = trits_from(&values);
            let mut packed = pack(radix, &trits).expect("pack partial");
            assert_eq!(packed.len(), 1);
            let byte = packed[0];

            let mutated = match radix {
                Radix::R3 => byte + 3u8.pow(rem as u32),
                Radix::R4 => byte | (0b11u8 << (2 * rem)),
                Radix::R16 => byte | 0xf0,
            };
            packed[0] = mutated;
            assert_ne!(mutated, byte);

            let error = radix
                .validate_packed(&packed, rem)
                .expect_err("non-canonical padding must be rejected");
            assert!(
                matches!(error, RadixError::NonCanonicalPadding { radix: found, .. } if found == radix),
                "unexpected validation error for {} rem {}: {error:?}",
                radix.name(),
                rem
            );

            let error = unpack(radix, &packed, rem).expect_err("unpack non-canonical");
            assert!(
                matches!(error, RadixError::NonCanonicalPadding { radix: found, .. } if found == radix)
            );
        }
    }
}

#[test]
fn invalid_used_slots_are_rejected() {
    let r4 = 0b11u8;
    let error = unpack(Radix::R4, &[r4], 4).expect_err("r4 slot 3");
    assert!(matches!(
        error,
        RadixError::InvalidPackedByte {
            radix: Radix::R4,
            ..
        }
    ));

    let r16 = 0x03u8;
    let error = unpack(Radix::R16, &[r16], 2).expect_err("r16 nibble 3");
    assert!(matches!(
        error,
        RadixError::InvalidPackedByte {
            radix: Radix::R16,
            ..
        }
    ));
}

#[test]
fn length_mismatch_is_structured() {
    let trits = trits_from(&[0, 1, 2, 0, 1, 2]);
    let packed = pack(Radix::R3, &trits).expect("pack");
    for actual in [packed.len() - 1, packed.len() + 1] {
        let bytes = vec![0u8; actual];
        let error = unpack(Radix::R3, &bytes, trits.len()).expect_err("length mismatch");
        assert!(matches!(
            error,
            RadixError::LengthMismatch {
                expected,
                actual: found
            } if expected == packed.len() && found == actual
        ));
    }
}

#[test]
fn metadata_and_name_parsing() {
    let names = [
        (Radix::R3, "r3", 3, 5, 1.6),
        (Radix::R4, "r4", 4, 4, 2.0),
        (Radix::R16, "r16", 16, 2, 4.0),
    ];
    for (radix, name, code, per_byte, bits) in names {
        assert_eq!(radix.name(), name);
        assert_eq!(radix.code(), code);
        assert_eq!(Radix::from_code(code), Some(radix));
        assert_eq!(Radix::from_name(name), Some(radix));
        assert_eq!(Radix::from_name(&name.to_ascii_uppercase()), Some(radix));
        assert_eq!(Radix::from_name(&code.to_string()), Some(radix));
        assert_eq!(radix.trits_per_byte(), per_byte);
        assert!((radix.bits_per_trit() - bits).abs() < f64::EPSILON);
    }
    assert_eq!(Radix::from_name("r5"), None);
    assert_eq!(Radix::from_name(""), None);
    assert_eq!(Radix::from_code(5), None);
}

#[test]
fn one_million_trits_roundtrip() {
    let mut rng = Rng::new(0x1_0000_0000);
    let values: Vec<u8> = (0..1_000_000).map(|_| rng.trit()).collect();
    let trits = trits_from(&values);
    for radix in Radix::ALL {
        let packed = pack(radix, &trits).expect("pack million");
        assert_eq!(packed.len(), radix.packed_len(trits.len()));
        radix
            .validate_packed(&packed, trits.len())
            .expect("million canonical");
        assert_eq!(
            unpack(radix, &packed, trits.len()).expect("unpack million"),
            trits,
            "million-trit mismatch for {}",
            radix.name()
        );
    }
}

#[test]
fn invalid_trit_constructors() {
    for value in 0..=2u8 {
        assert_eq!(Trit::from_u8(value).map(Trit::get), Some(value));
        assert!(Trit::try_from(value).is_ok());
    }
    for value in 3..=255u8 {
        assert!(Trit::from_u8(value).is_none());
        let error = Trit::try_from(value).expect_err("invalid trit");
        assert!(matches!(error, RadixError::InvalidTrit(found) if found == value));
    }
}
