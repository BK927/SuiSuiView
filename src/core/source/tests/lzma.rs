//! Method-14 fixtures generated from first-party bytes with Python zipfile.
use super::*;

const WITH_EOS: &[u8] = &[
    80, 75, 3, 4, 63, 0, 2, 0, 14, 0, 188, 113, 92, 93, 27, 208, 141, 34, 57, 0, 0, 0, 136, 19, 0,
    0, 10, 0, 0, 0, 112, 97, 103, 101, 45, 49, 46, 112, 110, 103, 9, 4, 5, 0, 93, 0, 0, 128, 0, 0,
    54, 30, 137, 221, 125, 204, 121, 235, 219, 100, 225, 251, 92, 45, 168, 163, 164, 124, 207, 85,
    156, 149, 44, 56, 123, 205, 79, 213, 94, 62, 54, 155, 88, 23, 231, 15, 250, 127, 220, 144, 79,
    255, 255, 7, 68, 0, 0, 80, 75, 1, 2, 63, 0, 63, 0, 2, 0, 14, 0, 188, 113, 92, 93, 27, 208, 141,
    34, 57, 0, 0, 0, 136, 19, 0, 0, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 128, 1, 0, 0, 0, 0, 112,
    97, 103, 101, 45, 49, 46, 112, 110, 103, 80, 75, 5, 6, 0, 0, 0, 0, 1, 0, 1, 0, 56, 0, 0, 0, 97,
    0, 0, 0, 0, 0,
];

const WITHOUT_EOS: &[u8] = &[
    80, 75, 3, 4, 63, 0, 0, 0, 14, 0, 188, 113, 92, 93, 27, 208, 141, 34, 51, 0, 0, 0, 136, 19, 0,
    0, 10, 0, 0, 0, 112, 97, 103, 101, 45, 49, 46, 112, 110, 103, 9, 4, 5, 0, 93, 0, 0, 128, 0, 0,
    54, 30, 137, 221, 125, 204, 121, 235, 219, 100, 225, 251, 92, 45, 168, 163, 164, 124, 207, 85,
    156, 149, 44, 56, 123, 205, 79, 213, 94, 62, 54, 155, 88, 23, 231, 15, 250, 127, 220, 144, 79,
    80, 75, 1, 2, 63, 0, 63, 0, 0, 0, 14, 0, 188, 113, 92, 93, 27, 208, 141, 34, 51, 0, 0, 0, 136,
    19, 0, 0, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 128, 1, 0, 0, 0, 0, 112, 97, 103, 101, 45, 49,
    46, 112, 110, 103, 80, 75, 5, 6, 0, 0, 0, 0, 1, 0, 1, 0, 56, 0, 0, 0, 91, 0, 0, 0, 0, 0,
];

fn open_fixture(bytes: &[u8], name: &str) -> ZipCbzSource {
    let dir = temp_test_dir(name);
    let path = dir.join("LZMA book.cbz");
    fs::write(&path, bytes).unwrap();
    ZipCbzSource::open(path).unwrap()
}

#[test]
fn lzma_full_and_prefix_reads_match_original_with_and_without_eos() {
    let expected = b"lzma-page-".repeat(500);
    for fixture in [WITH_EOS, WITHOUT_EOS] {
        let source = open_fixture(fixture, "lzma-valid");
        assert_eq!(
            source.page_read_hint(0).unwrap().compression_method,
            PageReadCompression::Lzma
        );
        assert_eq!(source.read_page_prefix(0, 17).unwrap(), &expected[..17]);
        assert_eq!(source.read_page_prefix(0, 0).unwrap(), b"");
        assert_eq!(source.read_page(0).unwrap(), expected);
        assert_eq!(
            source.read_page_prefix(0, expected.len() + 10).unwrap(),
            expected
        );
        assert_eq!(
            source
                .reopen_for_independent_reads()
                .unwrap()
                .unwrap()
                .read_page(0)
                .unwrap(),
            expected
        );
    }
}

#[test]
fn lzma_rejects_bad_properties_crc_truncation_and_oversize() {
    let central = WITH_EOS
        .windows(4)
        .position(|b| b == b"PK\x01\x02")
        .unwrap();
    let body = 30 + "page-1.png".len();
    let mut bad = WITH_EOS.to_vec();
    bad[body + 2] = 4; // Method-14 requires exactly five property bytes.
    assert!(open_fixture(&bad, "lzma-properties").read_page(0).is_err());
    assert!(open_fixture(&bad, "lzma-properties-prefix")
        .read_page_prefix(0, 17)
        .is_err());
    let mut bad = WITH_EOS.to_vec();
    bad[14] ^= 1;
    bad[central + 16] ^= 1;
    assert!(open_fixture(&bad, "lzma-crc").read_page(0).is_err());
    let mut bad = WITH_EOS.to_vec();
    let short_size = 12_u32.to_le_bytes();
    bad[18..22].copy_from_slice(&short_size);
    bad[central + 20..central + 24].copy_from_slice(&short_size);
    assert!(open_fixture(&bad, "lzma-truncated").read_page(0).is_err());
    for declared_size in [4999_u32, 5001] {
        let mut bad = WITH_EOS.to_vec();
        bad[22..26].copy_from_slice(&declared_size.to_le_bytes());
        bad[central + 24..central + 28].copy_from_slice(&declared_size.to_le_bytes());
        assert!(open_fixture(&bad, "lzma-wrong-size").read_page(0).is_err());
    }
    let mut bad = WITH_EOS.to_vec();
    let too_large = (super::super::MAX_SOURCE_PAGE_BYTES as u32 + 1).to_le_bytes();
    bad[22..26].copy_from_slice(&too_large);
    bad[central + 24..central + 28].copy_from_slice(&too_large);
    assert!(matches!(
        open_fixture(&bad, "lzma-limit").read_page(0),
        Err(SourceError::Unsupported(_))
    ));
}

#[test]
fn lzma_prefix_rejects_oversized_page_before_decoder_allocation() {
    let mut bad = WITH_EOS.to_vec();
    let central = bad.windows(4).position(|b| b == b"PK\x01\x02").unwrap();
    let too_large = (super::super::MAX_SOURCE_PAGE_BYTES as u32 + 1).to_le_bytes();
    bad[22..26].copy_from_slice(&too_large);
    bad[central + 24..central + 28].copy_from_slice(&too_large);
    let source = open_fixture(&bad, "lzma-prefix-limit");
    assert!(matches!(
        source.read_page_prefix(0, 17),
        Err(SourceError::Unsupported(_))
    ));
}
