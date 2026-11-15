use super::*;
use crate::image_decode::camera_raw::test_files;

/// 小端 TIFF：IFD0（厂商、型号、EXIF 指针、GPS 指针）+ EXIF 目录 + GPS 目录，值都放在后面的数据区。
fn exif_tiff() -> Vec<u8> {
    struct Entry {
        tag: u16,
        field_type: u16,
        count: u32,
        data: Vec<u8>,
    }
    let ascii = |tag, text: &str| Entry {
        tag,
        field_type: 2,
        count: text.len() as u32 + 1,
        data: [text.as_bytes(), b"\0"].concat(),
    };
    let rationals = |tag, values: &[(u32, u32)]| Entry {
        tag,
        field_type: 5,
        count: values.len() as u32,
        data: values
            .iter()
            .flat_map(|(n, d)| [n.to_le_bytes(), d.to_le_bytes()].concat())
            .collect(),
    };
    let short = |tag, value: u16| Entry {
        tag,
        field_type: 3,
        count: 1,
        data: value.to_le_bytes().to_vec(),
    };
    let pointer = |tag| Entry {
        tag,
        field_type: 4,
        count: 1,
        data: Vec::new(),
    };
    let directories: Vec<Vec<Entry>> = vec![
        vec![
            ascii(TAG_MAKE, "Canon"),
            ascii(TAG_MODEL, "Canon EOS R5"),
            pointer(TAG_EXIF_IFD),
            pointer(TAG_GPS_IFD),
        ],
        vec![
            rationals(TAG_EXPOSURE_TIME, &[(1, 250)]),
            rationals(TAG_F_NUMBER, &[(28, 10)]),
            short(TAG_ISO, 400),
            ascii(TAG_DATE_TIME_ORIGINAL, "2024:05:01 12:34:56"),
            rationals(TAG_FOCAL_LENGTH, &[(50, 1)]),
            ascii(TAG_LENS_MODEL, "RF50mm F1.8 STM"),
        ],
        vec![
            ascii(TAG_GPS_LATITUDE_REF, "N"),
            rationals(TAG_GPS_LATITUDE, &[(39, 1), (54, 1), (0, 1)]),
            ascii(TAG_GPS_LONGITUDE_REF, "W"),
            rationals(TAG_GPS_LONGITUDE, &[(116, 1), (30, 1), (0, 1)]),
        ],
    ];
    let directory_len = |entries: &Vec<Entry>| 2 + entries.len() * 12 + 4;
    let mut directory_offsets = Vec::new();
    let mut position = 8;
    for entries in &directories {
        directory_offsets.push(position);
        position += directory_len(entries);
    }
    let mut data_position = position;
    let mut tables = Vec::new();
    let mut data: Vec<u8> = Vec::new();
    for (index, entries) in directories.iter().enumerate() {
        let mut table = (entries.len() as u16).to_le_bytes().to_vec();
        for entry in entries {
            table.extend(entry.tag.to_le_bytes());
            table.extend(entry.field_type.to_le_bytes());
            table.extend(entry.count.to_le_bytes());
            let value: u32 = match entry.tag {
                TAG_EXIF_IFD if index == 0 => directory_offsets[1] as u32,
                TAG_GPS_IFD if index == 0 => directory_offsets[2] as u32,
                _ if entry.data.len() <= 4 => {
                    let mut inline = entry.data.clone();
                    inline.resize(4, 0);
                    u32::from_le_bytes(inline.try_into().unwrap())
                }
                _ => {
                    let offset = data_position as u32;
                    data_position += entry.data.len();
                    data.extend(&entry.data);
                    offset
                }
            };
            table.extend(value.to_le_bytes());
        }
        table.extend(0u32.to_le_bytes());
        tables.push(table);
    }
    let mut bytes = b"II*\0\x08\0\0\0".to_vec();
    for table in tables {
        bytes.extend(table);
    }
    bytes.extend(data);
    bytes
}

#[test]
fn exif_block_gives_camera_exposure_and_location() {
    let mut facts = ImageFacts::default();
    read_exif_block(&[b"Exif\0\0".as_slice(), &exif_tiff()].concat(), &mut facts);
    assert_eq!(facts.camera.as_deref(), Some("Canon EOS R5"));
    assert_eq!(facts.lens.as_deref(), Some("RF50mm F1.8 STM"));
    assert_eq!(facts.taken_at.as_deref(), Some("2024-05-01 12:34:56"));
    assert_eq!(facts.exposure, Some((1, 250)));
    assert_eq!(facts.f_number, Some(2.8));
    assert_eq!(facts.iso, Some(400));
    assert_eq!(facts.focal_length_mm, Some(50.0));
    let (latitude, longitude) = facts.location.expect("location");
    assert!((latitude - 39.9).abs() < 1e-9);
    assert!((longitude + 116.5).abs() < 1e-9);
}

#[test]
fn camera_name_drops_the_repeated_brand() {
    let name = |make: &str, model: &str| camera_name(Some(make.to_owned()), Some(model.to_owned()));
    assert_eq!(
        name("NIKON CORPORATION", "NIKON D850").as_deref(),
        Some("NIKON D850")
    );
    assert_eq!(name("SONY", "ILCE-7M4").as_deref(), Some("SONY ILCE-7M4"));
    assert_eq!(
        camera_name(None, Some("X100V".to_owned())).as_deref(),
        Some("X100V")
    );
}

#[test]
fn builtin_picture_reports_format_and_depth() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("deep.png");
    image::ImageBuffer::<image::Rgb<u16>, _>::from_pixel(2, 2, image::Rgb([1, 2, 3]))
        .save(&path)
        .expect("save png");
    let facts = image_facts(&path, RasterSource::Picture);
    assert_eq!(facts.format.as_deref(), Some("PNG"));
    assert_eq!(facts.bits_per_channel, Some(16));

    let raw = directory.path().join("shot.nef");
    std::fs::write(
        &raw,
        test_files::tiff_raw(1, &test_files::jpeg(40, 30), &test_files::jpeg(160, 120)),
    )
    .expect("write raw");
    assert_eq!(
        image_facts(&raw, RasterSource::CameraRaw).format.as_deref(),
        Some("RAW (NEF)")
    );
}

#[test]
fn icc_v2_and_v4_descriptions_are_read() {
    fn profile(tag: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0; 128];
        bytes.extend(1u32.to_be_bytes());
        bytes.extend(b"desc");
        bytes.extend(144u32.to_be_bytes());
        bytes.extend((tag.len() as u32).to_be_bytes());
        bytes.extend(tag);
        bytes
    }
    let mut v2 = b"desc\0\0\0\0".to_vec();
    v2.extend(5u32.to_be_bytes());
    v2.extend(b"sRGB\0");
    assert_eq!(icc_description(&profile(&v2)).as_deref(), Some("sRGB"));

    let text: Vec<u8> = "Display P3"
        .encode_utf16()
        .flat_map(u16::to_be_bytes)
        .collect();
    let mut v4 = b"mluc\0\0\0\0".to_vec();
    v4.extend(1u32.to_be_bytes());
    v4.extend(12u32.to_be_bytes());
    v4.extend(b"enUS");
    v4.extend((text.len() as u32).to_be_bytes());
    v4.extend(28u32.to_be_bytes());
    v4.extend(&text);
    assert_eq!(
        icc_description(&profile(&v4)).as_deref(),
        Some("Display P3")
    );
}
