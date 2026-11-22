//! Page container fuzzing: truncation, corruption, and boundary payloads.

use tritdb_page::{
    crc32, crc32_parts, Page, PageError, PageKind, HEADER_SIZE, MAGIC, MAX_PAYLOAD, PAGE_SIZE,
    VERSION,
};
use tritdb_testkit::Rng;

fn payload(len: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    (0..len).map(|_| rng.next_u32() as u8).collect()
}

#[test]
fn every_truncation_is_rejected_without_panic() {
    let page = Page::new(PageKind::Data, &payload(MAX_PAYLOAD, 7)).expect("page");
    let raw = page.as_bytes();
    for cut in 0..PAGE_SIZE {
        let error = Page::parse_slice(&raw[..cut]).expect_err("short page must fail");
        assert!(
            matches!(error, PageError::Truncated { expected, actual } if expected == PAGE_SIZE && actual == cut),
            "unexpected error at cut {cut}: {error:?}"
        );
    }
    assert!(Page::parse_slice(raw).is_ok());
    let error = Page::parse_slice(&[0u8; PAGE_SIZE + 1]).expect_err("long page must fail");
    assert!(matches!(
        error,
        PageError::Truncated {
            expected: PAGE_SIZE,
            actual: 8193
        }
    ));
}

#[test]
fn every_single_byte_corruption_is_detected() {
    let mut page = Page::new(PageKind::Index, &payload(MAX_PAYLOAD, 11))
        .expect("page")
        .into_bytes();
    for index in 0..PAGE_SIZE {
        let original = page[index];
        page[index] ^= 0xff;
        let error = Page::parse(&page).expect_err("corruption must be detected");
        match index {
            0..=11 => assert!(
                matches!(
                    error,
                    PageError::BadMagic { .. }
                        | PageError::BadVersion { .. }
                        | PageError::BadKind { .. }
                        | PageError::BadFlags { .. }
                        | PageError::PayloadTooLarge { .. }
                        | PageError::ChecksumMismatch { .. }
                ),
                "unexpected header error at byte {index}: {error:?}"
            ),
            _ => assert!(
                matches!(error, PageError::ChecksumMismatch { .. }),
                "unexpected payload/CRC error at byte {index}: {error:?}"
            ),
        }
        page[index] = original;
    }
}

#[test]
fn every_payload_length_roundtrips() {
    for len in 0..=MAX_PAYLOAD {
        let bytes = payload(len, 0xA11CE + len as u64);
        let page = Page::new(PageKind::Manifest, &bytes)
            .unwrap_or_else(|error| panic!("len {len}: {error}"));
        let view = Page::parse(page.as_bytes()).expect("parse");
        assert_eq!(view.kind(), PageKind::Manifest);
        assert_eq!(view.payload_len(), len);
        assert_eq!(view.payload(), bytes.as_slice());
        assert_eq!(page.as_bytes().len(), PAGE_SIZE);
    }
}

#[test]
fn payload_too_large_is_rejected() {
    let error = Page::new(PageKind::Data, &vec![0u8; MAX_PAYLOAD + 1])
        .err()
        .expect("payload too large");
    assert!(matches!(
        error,
        PageError::PayloadTooLarge {
            len,
            max
        } if len == MAX_PAYLOAD + 1 && max == MAX_PAYLOAD
    ));
}

#[test]
fn header_layout_matches_spec() {
    let bytes = Page::new(PageKind::Data, b"abc")
        .expect("page")
        .into_bytes();
    assert_eq!(&bytes[0..4], &MAGIC);
    assert_eq!(bytes[4], VERSION);
    assert_eq!(bytes[5], PageKind::Data.code());
    assert_eq!(&bytes[6..8], &[0, 0]);
    assert_eq!(&bytes[8..12], &3u32.to_le_bytes());
    let expected_crc = crc32_parts(&bytes[0..12], &bytes[HEADER_SIZE..HEADER_SIZE + 3]);
    assert_eq!(&bytes[12..16], &expected_crc.to_le_bytes());
    assert_eq!(&bytes[16..19], b"abc");
    assert!(bytes[19..].iter().all(|byte| *byte == 0));
}

#[test]
fn all_page_kinds_roundtrip() {
    for kind in [PageKind::Data, PageKind::Index, PageKind::Manifest] {
        assert_eq!(PageKind::from_u8(kind.code()), Ok(kind));
        let page = Page::new(kind, b"kind").expect("page");
        let view = Page::parse(page.as_bytes()).expect("parse");
        assert_eq!(view.kind(), kind);
    }
    let error = PageKind::from_u8(0).expect_err("kind zero");
    assert!(matches!(error, PageError::BadKind { found: 0 }));
    let error = PageKind::from_u8(4).expect_err("kind four");
    assert!(matches!(error, PageError::BadKind { found: 4 }));
}

#[test]
fn crc_matches_known_vector_and_incremental_parts() {
    assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    assert_eq!(crc32(b""), 0);
    assert_eq!(crc32_parts(b"1234", b"56789"), crc32(b"123456789"));
    let mut rng = Rng::new(0x5EED);
    for _ in 0..100 {
        let first: Vec<u8> = (0..rng.below(64)).map(|_| rng.next_u32() as u8).collect();
        let second: Vec<u8> = (0..rng.below(64)).map(|_| rng.next_u32() as u8).collect();
        let mut combined = first.clone();
        combined.extend_from_slice(&second);
        assert_eq!(crc32_parts(&first, &second), crc32(&combined));
    }
}

#[test]
fn zeroed_crc_is_a_half_write() {
    let mut page = Page::new(PageKind::Data, b"payload")
        .expect("page")
        .into_bytes();
    page[12..16].fill(0);
    let error = Page::parse(&page).expect_err("zero CRC");
    assert!(matches!(error, PageError::ChecksumMismatch { .. }));
}

#[test]
fn parse_rejects_bad_magic_and_version_before_crc() {
    let mut page = Page::new(PageKind::Data, b"payload")
        .expect("page")
        .into_bytes();
    page[0] = b'X';
    assert!(matches!(
        Page::parse(&page),
        Err(PageError::BadMagic { found }) if found[0] == b'X'
    ));

    let mut page = Page::new(PageKind::Data, b"payload")
        .expect("page")
        .into_bytes();
    page[4] = 9;
    assert!(matches!(
        Page::parse(&page),
        Err(PageError::BadVersion { found: 9 })
    ));
}
