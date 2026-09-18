//! PNG codec wrapper (png 0.18): every input decodes to straight RGBA8.
//!
//! Artists save sheets in whatever their editor picks: indexed, grey,
//! grey+alpha, RGB, RGBA, 1/2/4/8/16-bit, interlaced. `EXPAND | STRIP_16 |
//! ALPHA` normalises all of those to 8-bit RGBA or 8-bit grey+alpha (png 0.18
//! has no grey-to-RGB transform), and the grey forms are widened here.

use std::fmt;
use std::io::Cursor;

/// Largest accepted sheet edge in pixels (4096x4096 RGBA = 64 MiB).
pub const MAX_SHEET_DIM: u32 = 4096;

/// A decoded RGBA8 image, row-major, `width * height * 4` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl RgbaImage {
    /// A fully transparent (all-zero) image.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            rgba: vec![0; width as usize * height as usize * 4],
        }
    }

    /// Pixel at `(x, y)`. Panics when out of bounds.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }

    /// Set the pixel at `(x, y)`. Panics when out of bounds.
    pub fn set_pixel(&mut self, x: u32, y: u32, px: [u8; 4]) {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        self.rgba[i..i + 4].copy_from_slice(&px);
    }
}

/// PNG decode/encode failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PngError {
    /// The byte stream is not a decodable PNG.
    Decode(String),
    /// Encoding failed (bad dimensions, text chunk, ...).
    Encode(String),
    /// The image exceeds [`MAX_SHEET_DIM`] on an edge.
    TooLarge { width: u32, height: u32 },
    /// The buffer handed to the encoder has the wrong length.
    BadBuffer { want: usize, got: usize },
    /// The decoder produced a layout this wrapper does not handle.
    Unsupported(String),
}

impl fmt::Display for PngError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PngError::Decode(m) => write!(f, "not a readable PNG: {m}"),
            PngError::Encode(m) => write!(f, "PNG encode failed: {m}"),
            PngError::TooLarge { width, height } => write!(
                f,
                "image is {width}x{height}; sheets may be at most {MAX_SHEET_DIM}x{MAX_SHEET_DIM}"
            ),
            PngError::BadBuffer { want, got } => {
                write!(f, "RGBA buffer is {got} bytes, expected {want}")
            }
            PngError::Unsupported(m) => write!(f, "unsupported PNG layout: {m}"),
        }
    }
}

impl std::error::Error for PngError {}

/// Decode any PNG into RGBA8.
///
/// # Errors
/// [`PngError::Decode`] for malformed data, [`PngError::TooLarge`] when an
/// edge exceeds [`MAX_SHEET_DIM`].
pub fn decode_png_rgba(bytes: &[u8]) -> Result<RgbaImage, PngError> {
    // 16-bit RGBA at the max size before STRIP_16, plus slack.
    let limits = png::Limits {
        bytes: (MAX_SHEET_DIM as usize) * (MAX_SHEET_DIM as usize) * 8 + (1 << 20),
    };
    let mut decoder = png::Decoder::new_with_limits(Cursor::new(bytes), limits);
    decoder.set_transformations(
        png::Transformations::EXPAND | png::Transformations::STRIP_16 | png::Transformations::ALPHA,
    );
    let mut reader = decoder
        .read_info()
        .map_err(|e| PngError::Decode(e.to_string()))?;
    let (width, height) = {
        let info = reader.info();
        (info.width, info.height)
    };
    if width > MAX_SHEET_DIM || height > MAX_SHEET_DIM {
        return Err(PngError::TooLarge { width, height });
    }
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| PngError::Decode("output buffer size overflows".into()))?;
    let mut buf = vec![0u8; size];
    let out = reader
        .next_frame(&mut buf)
        .map_err(|e| PngError::Decode(e.to_string()))?;
    buf.truncate(out.buffer_size());
    let (color, depth) = reader.output_color_type();
    if depth != png::BitDepth::Eight {
        return Err(PngError::Unsupported(format!(
            "bit depth {depth:?} after STRIP_16"
        )));
    }
    let pixels = width as usize * height as usize;
    let rgba = match color {
        png::ColorType::Rgba => buf,
        png::ColorType::GrayscaleAlpha => {
            let mut v = Vec::with_capacity(pixels * 4);
            for ga in buf.chunks_exact(2) {
                v.extend_from_slice(&[ga[0], ga[0], ga[0], ga[1]]);
            }
            v
        }
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 0xFF]).collect(),
        png::ColorType::Rgb => buf
            .chunks_exact(3)
            .flat_map(|c| [c[0], c[1], c[2], 0xFF])
            .collect(),
        png::ColorType::Indexed => {
            return Err(PngError::Unsupported(
                "indexed output after EXPAND".to_string(),
            ))
        }
    };
    if rgba.len() != pixels * 4 {
        return Err(PngError::Decode(format!(
            "decoded {} bytes for a {width}x{height} image",
            rgba.len()
        )));
    }
    Ok(RgbaImage {
        width,
        height,
        rgba,
    })
}

/// Encode RGBA8 as an 8-bit RGBA PNG, adding one tEXt chunk per `text` pair.
///
/// # Errors
/// [`PngError::BadBuffer`] when `rgba` is not `width * height * 4` bytes,
/// [`PngError::TooLarge`] beyond [`MAX_SHEET_DIM`], [`PngError::Encode`] on
/// encoder failure.
pub fn encode_png_rgba(
    width: u32,
    height: u32,
    rgba: &[u8],
    text: &[(&str, &str)],
) -> Result<Vec<u8>, PngError> {
    let want = width as usize * height as usize * 4;
    if rgba.len() != want {
        return Err(PngError::BadBuffer {
            want,
            got: rgba.len(),
        });
    }
    if width > MAX_SHEET_DIM || height > MAX_SHEET_DIM {
        return Err(PngError::TooLarge { width, height });
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        for (k, v) in text {
            enc.add_text_chunk((*k).to_string(), (*v).to_string())
                .map_err(|e| PngError::Encode(e.to_string()))?;
        }
        let mut writer = enc
            .write_header()
            .map_err(|e| PngError::Encode(e.to_string()))?;
        writer
            .write_image_data(rgba)
            .map_err(|e| PngError::Encode(e.to_string()))?;
        writer
            .finish()
            .map_err(|e| PngError::Encode(e.to_string()))?;
    }
    Ok(out)
}
