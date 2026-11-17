//! 系统 libjpeg-turbo 逐行解 JPEG 的 Rust 一侧（C 包装在 `jpeg_rows.c`，为什么用它也写在那里）。

use std::ffi::{c_char, c_int, c_uint, CStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::ptr::NonNull;

use super::row_reader::RowReader;
use super::{decoding_error, ImageDecodeError};

#[repr(C)]
struct RawJpeg {
    _private: [u8; 0],
}

const OK: c_int = 0;
const UNSUPPORTED: c_int = 2;
const MESSAGE_LEN: usize = 256;

extern "C" {
    fn bennu_jpeg_open(
        path: *const c_char,
        out: *mut *mut RawJpeg,
        width: *mut c_uint,
        height: *mut c_uint,
        message: *mut c_char,
        message_len: usize,
    ) -> c_int;
    fn bennu_jpeg_read_rows(
        jpeg: *mut RawJpeg,
        rgba: *mut u8,
        max_rows: c_uint,
        rows_read: *mut c_uint,
        message: *mut c_char,
        message_len: usize,
    ) -> c_int;
    fn bennu_jpeg_close(jpeg: *mut RawJpeg);
}

pub(super) struct JpegRows {
    jpeg: NonNull<RawJpeg>,
    width: u32,
    height: u32,
}

// 解码状态只归这一个读取器，跟着它在线程间移动，不共享。
unsafe impl Send for JpegRows {}

impl JpegRows {
    /// CMYK/YCCK 的 JPEG 返回 `None`，交给整张解码（libjpeg 不转 RGB）。
    pub(super) fn open(path: &Path) -> Result<Option<Self>, ImageDecodeError> {
        let c_path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| failed("the path contains a NUL byte"))?;
        let mut jpeg = std::ptr::null_mut();
        let (mut width, mut height) = (0, 0);
        let mut message = [0 as c_char; MESSAGE_LEN];
        // SAFETY: 参数都指向本函数里活着的值；成功时 `jpeg` 是 C 侧分配、由 `Drop` 释放的对象。
        let status = unsafe {
            bennu_jpeg_open(
                c_path.as_ptr(),
                &mut jpeg,
                &mut width,
                &mut height,
                message.as_mut_ptr(),
                MESSAGE_LEN,
            )
        };
        match status {
            OK => Ok(Some(Self {
                jpeg: NonNull::new(jpeg).expect("libjpeg wrapper returned no decoder"),
                width,
                height,
            })),
            UNSUPPORTED => Ok(None),
            _ => Err(failed(&message_text(&message))),
        }
    }
}

impl RowReader for JpegRows {
    fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn read_rows(&mut self, rgba: &mut [u8]) -> Result<usize, ImageDecodeError> {
        let max_rows = rgba.len() / (self.width as usize * 4);
        let mut rows_read = 0;
        let mut message = [0 as c_char; MESSAGE_LEN];
        // SAFETY: `rgba` 至少有 `max_rows` 整行；解码对象由 `self` 独占。
        let status = unsafe {
            bennu_jpeg_read_rows(
                self.jpeg.as_ptr(),
                rgba.as_mut_ptr(),
                c_uint::try_from(max_rows).unwrap_or(c_uint::MAX),
                &mut rows_read,
                message.as_mut_ptr(),
                MESSAGE_LEN,
            )
        };
        if status == OK {
            Ok(rows_read as usize)
        } else {
            Err(failed(&message_text(&message)))
        }
    }
}

impl Drop for JpegRows {
    fn drop(&mut self) {
        // SAFETY: `jpeg` 来自 `bennu_jpeg_open`，只在这里释放一次。
        unsafe { bennu_jpeg_close(self.jpeg.as_ptr()) }
    }
}

fn message_text(message: &[c_char]) -> String {
    // SAFETY: C 侧总是写入以 NUL 结尾的文字（copy_message 截断后补 NUL）。
    unsafe { CStr::from_ptr(message.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

fn failed(message: &str) -> ImageDecodeError {
    decoding_error(image::ImageFormat::Jpeg, message.to_owned())
}
