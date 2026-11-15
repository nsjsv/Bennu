use super::*;

const KIB: u64 = BYTES_PER_KIB;
const MIB: u64 = KIB * KIB;
const GIB: u64 = MIB * KIB;

#[test]
fn file_sizes_are_written_the_kde_way() {
    for (bytes, regular, compact) in [
        (0, "0 B", "0B"),
        (1023, "1023 B", "1023B"),
        (1024, "1.0 KiB", "1.0K"),
        (15_000_000, "14.3 MiB", "14.3M"),
        (47_185_920, "45.0 MiB", "45.0M"),
        (1 << 40, "1.0 TiB", "1.0T"),
        (u64::MAX, "16.0 EiB", "16.0E"),
    ] {
        assert_eq!(format_file_size(bytes), regular);
        assert_eq!(format_file_size_compact(bytes), compact);
    }
}

#[test]
fn every_spelling_of_a_unit_counts_in_1024s() {
    for spelling in ["2M", "2MB", "2MiB", "2 mb", "2 mib", "2 Mi", " 2m "] {
        assert_eq!(parse_file_size(spelling), Ok(2 * MIB), "{spelling:?}");
    }
    assert_eq!(parse_file_size("500kb"), Ok(500 * KIB));
    assert_eq!(parse_file_size("1.5GB"), Ok(GIB + GIB / 2));
    assert_eq!(parse_file_size("5 TB"), Ok(5 << 40));
    assert_eq!(parse_file_size("1 PiB"), Ok(1 << 50));
    assert_eq!(parse_file_size("1 E"), Ok(1 << 60));
    // 不写单位是字节。
    for spelling in ["42", "42 B", "42 bytes", "42 字节"] {
        assert_eq!(parse_file_size(spelling), Ok(42), "{spelling:?}");
    }
    // 小数向下取整到字节。
    assert_eq!(parse_file_size("0.5KB"), Ok(KIB / 2));
    assert_eq!(parse_file_size("1.25 KiB"), Ok(KIB + KIB / 4));
}

#[test]
fn malformed_sizes_and_unknown_units_are_rejected() {
    assert_eq!(
        parse_file_size("5 XB"),
        Err(FileSizeInputError::UnknownUnit("XB".to_owned()))
    );
    for invalid in [
        "-5", "-1 B", "1M2", "abc", "4.", ".5", "1.2.3", "", "   ", "5 MiBs",
    ] {
        assert!(parse_file_size(invalid).is_err(), "{invalid:?}");
    }
}

#[test]
fn sizes_beyond_u64_bytes_are_too_large() {
    assert_eq!(parse_file_size("18446744073709551615"), Ok(u64::MAX));
    assert!(matches!(
        parse_file_size("18446744073709551616"),
        Err(FileSizeInputError::TooLarge(_))
    ));
    assert!(parse_file_size("17179869184 GB").is_err());
    // 38 位小数本身在 u128 内可表示，乘上 GiB 会溢出：报错，不能 panic 或回绕。
    assert!(parse_file_size(&format!("0.{}GB", "9".repeat(38))).is_err());
    assert_eq!(parse_file_size(&format!("0.{}1", "0".repeat(37))), Ok(0));
}

#[test]
fn exact_sizes_read_back_unchanged() {
    for (bytes, written) in [
        (0, "0 B"),
        (1000, "1000 B"),
        (8 * MIB, "8 MiB"),
        (MIB + MIB / 4, "1280 KiB"),
        (3 * GIB, "3 GiB"),
    ] {
        assert_eq!(format_file_size_exact(bytes), written);
        assert_eq!(parse_file_size(written), Ok(bytes));
    }
}
