//! ISO 媒体（盒子套盒子）结构的读取：CR3、HEIF/AVIF、JPEG XL 容器都是这种结构。
//! 只按偏移读盒子头，不整份读文件。

use std::fs::File;
use std::io;

use super::tiff::ByteSource;

pub(super) fn descend(
    file: &mut File,
    parent: &IsoBox,
    path: &[&[u8; 4]],
) -> io::Result<Option<IsoBox>> {
    let mut current = IsoBox {
        kind: parent.kind,
        payload: parent.payload,
        end: parent.end,
    };
    for kind in path {
        let Some(child) = iso_boxes(file, current.payload, current.end)?
            .into_iter()
            .find(|child| &child.kind == *kind)
        else {
            return Ok(None);
        };
        current = child;
    }
    Ok(Some(current))
}

pub(super) struct IsoBox {
    pub(super) kind: [u8; 4],
    pub(super) payload: u64,
    pub(super) end: u64,
}

pub(super) fn iso_boxes(file: &mut File, mut offset: u64, end: u64) -> io::Result<Vec<IsoBox>> {
    let mut boxes = Vec::new();
    while offset + 8 <= end {
        let mut header = [0; 16];
        file.read_exact_at(offset, &mut header[..8])?;
        let size = u64::from(u32::from_be_bytes([
            header[0], header[1], header[2], header[3],
        ]));
        let kind = [header[4], header[5], header[6], header[7]];
        let (size, header_len) = match size {
            0 => (end - offset, 8),
            1 => {
                file.read_exact_at(offset + 8, &mut header[8..16])?;
                let large = u64::from_be_bytes([
                    header[8], header[9], header[10], header[11], header[12], header[13],
                    header[14], header[15],
                ]);
                (large, 16)
            }
            size => (size, 8),
        };
        if size < header_len || offset + size > end {
            break;
        }
        boxes.push(IsoBox {
            kind,
            payload: offset + header_len,
            end: offset + size,
        });
        offset += size;
    }
    Ok(boxes)
}

pub(super) fn box_uuid(file: &mut File, uuid_box: &IsoBox) -> io::Result<[u8; 16]> {
    let mut uuid = [0; 16];
    if uuid_box.end - uuid_box.payload >= 16 {
        file.read_exact_at(uuid_box.payload, &mut uuid)?;
    }
    Ok(uuid)
}
