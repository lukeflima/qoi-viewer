mod utils;

use core::panic;

use wasm_bindgen::prelude::*;
use wasm_bindgen::Clamped;
use web_sys::ImageData;

// When the `wee_alloc` feature is enabled, use `wee_alloc` as the global
// allocator.
#[cfg(feature = "wee_alloc")]
#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;

#[wasm_bindgen]
extern "C" {
    fn alert(s: &str);
    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);
}

#[allow(unused_macros)]
macro_rules! console_log {
    // Note that this is using the `log` function imported above during
    // `bare_bones`
    ($($t:tt)*) => (log(&format_args!($($t)*).to_string()))
}

#[derive(Debug, Default, Clone, Copy)]
pub struct QoiHeader {
    magic: u32,     // magic bytes "qoif"
    width: u32,     // image width in pixels (BE)
    height: u32,    // image height in pixels (BE)
    channels: u8,   // 3 = RGB, 4 = RGBA
    colorspace: u8, // 0 = sRGB with linear alpha, 1 = all channels linear
}

#[derive(Debug, Default, Clone, Copy)]
struct QoiColor {
    r: u8,
    g: u8,
    b: u8,
    a: u8,
}

impl QoiColor {
    fn hash(&self) -> usize {
        (self.r * 3 + self.g * 5 + self.b * 7 + self.a * 11) as usize
    }
}

impl From<u32> for QoiColor {
    fn from(color: u32) -> Self {
        Self {
            r: ((color & 0xff000000) >> 24) as u8,
            g: ((color & 0x00ff0000) >> 16) as u8,
            b: ((color & 0x0000ff00) >> 8) as u8,
            a: (color & 0x000000ff) as u8,
        }
    }
}

const QOI_MAGIC: u32 =
    (('q' as u32) << 24) | (('o' as u32) << 16) | (('i' as u32) << 8) | 'f' as u32;
const QOI_PIXELS_MAX: u32 = 400000000;
const QOI_HEADER_SIZE: usize = 14;
const QOI_END_SEGMENT_SIZE: usize = 8;

enum QoiOp {
    Index(usize),
    Diff(QoiColor),
    Luma(u8),
    Run(usize),
    Rgb,
    Rgba,
    Unknown,
}

impl From<u8> for QoiOp {
    fn from(b: u8) -> Self {
        if b == 0xfe {
            QoiOp::Rgb
        } else if b == 0xff {
            QoiOp::Rgba
        } else if (b & 0xc0) == 0x00 {
            QoiOp::Index(b as usize)
        } else if (b & 0xc0) == 0x40 {
            QoiOp::Diff(QoiColor {
                r: ((b >> 4) & 0x03) - 2,
                g: ((b >> 2) & 0x03) - 2,
                b: (b & 0x03) - 2,
                a: 0,
            })
        } else if (b & 0xc0) == 0x80 {
            QoiOp::Luma((b & 0x3f) - 32)
        } else if (b & 0xc0) == 0xc0 {
            QoiOp::Run((b & 0x3f) as usize)
        } else {
            QoiOp::Unknown
        }
    }
}

fn read_32(bytes: &[u8], offset: &mut usize) -> u32 {
    let a = bytes[*offset] as u32;
    let b = bytes[*offset + 1] as u32;
    let c = bytes[*offset + 2] as u32;
    let d = bytes[*offset + 3] as u32;
    *offset += 4;
    a << 24 | b << 16 | c << 8 | d
}

fn read_8(bytes: &[u8], offset: &mut usize) -> u8 {
    let a = bytes[*offset];
    *offset += 1;
    a
}

#[derive(Default, Debug, Clone)]
#[wasm_bindgen]
pub struct QoiImage {
    header: QoiHeader,
    bytes: Vec<u8>,
}
#[wasm_bindgen]
impl QoiImage {
    #[wasm_bindgen(constructor)]
    pub fn new() -> QoiImage {
        QoiImage {
            header: Default::default(),
            bytes: Default::default(),
        }
    }

    pub fn get_width(&self) -> u32 {
        self.header.width
    }
    pub fn get_height(&self) -> u32 {
        self.header.height
    }
    pub fn get_channels(&self) -> u8 {
        self.header.channels
    }
    pub fn get_bytes(&self) -> Vec<u8> {
        self.bytes.clone()
    }
}

#[wasm_bindgen]
pub fn decode_qoi(bytes: &[u8]) -> Result<ImageData, JsValue> {
    utils::set_panic_hook();
    let size = bytes.len();
    if size < QOI_HEADER_SIZE {
        panic!("File too small to be a valid QOI image");
    }

    let mut index = 0_usize;
    let header = QoiHeader {
        magic: read_32(bytes, &mut index),
        width: read_32(bytes, &mut index),
        height: read_32(bytes, &mut index),
        channels: read_8(bytes, &mut index),
        colorspace: read_8(bytes, &mut index),
    };

    if header.width == 0
        || header.height == 0
        || header.channels < 3
        || header.channels > 4
        || header.colorspace > 1
        || header.magic != QOI_MAGIC
        || header.height >= QOI_PIXELS_MAX / header.width
    {
        panic!("Not a valid QOI image");
    }

    // Can only construct ImageData from RGBA
    let px_len: usize = (header.width * header.height * 4) as usize;
    let mut pixels: Vec<u8> = vec![0; px_len];

    let mut prev_color = QoiColor::from(0x000000FF);
    let mut seen_colors: [QoiColor; 64] = [QoiColor::from(0x000000FF); 64];
    let mut run: usize = 0;
    let chunks_len: usize = size - QOI_END_SEGMENT_SIZE;
    for px_pos in (0..px_len).step_by(4) {
        if run > 0 {
            run -= 1;
        } else if index < chunks_len {
            let b1 = read_8(bytes, &mut index);

            match QoiOp::from(b1) {
                QoiOp::Rgb => {
                    prev_color.r = read_8(bytes, &mut index);
                    prev_color.g = read_8(bytes, &mut index);
                    prev_color.b = read_8(bytes, &mut index);
                }
                QoiOp::Rgba => {
                    prev_color.r = read_8(bytes, &mut index);
                    prev_color.g = read_8(bytes, &mut index);
                    prev_color.b = read_8(bytes, &mut index);
                    prev_color.a = read_8(bytes, &mut index);
                }
                QoiOp::Index(index) => {
                    prev_color = seen_colors[index];
                }
                QoiOp::Diff(c) => {
                    prev_color.r += c.r;
                    prev_color.g += c.g;
                    prev_color.b += c.b
                }
                QoiOp::Luma(vg) => {
                    let b2 = read_8(bytes, &mut index);
                    prev_color.r += vg - 8 + ((b2 >> 4) & 0x0f);
                    prev_color.g += vg;
                    prev_color.b += vg - 8 + (b2 & 0x0f);
                }
                QoiOp::Run(runs) => {
                    run = runs;
                }
                QoiOp::Unknown => {
                    unreachable!("Not a valid QoiOp.")
                }
            }

            seen_colors[prev_color.hash() % 64] = prev_color;
        }

        pixels[px_pos] = prev_color.r;
        pixels[px_pos + 1] = prev_color.g;
        pixels[px_pos + 2] = prev_color.b;
        pixels[px_pos + 3] = if header.channels == 4 {
            prev_color.a
        } else {
            255
        }
    }

    ImageData::new_with_u8_clamped_array_and_sh(Clamped(&pixels), header.width, header.height)
}
