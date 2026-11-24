use fast_image_resize as fir;
use image::DynamicImage;
use image::GenericImageView;
use image::GrayImage;
use image::codecs::jpeg::JpegEncoder;

use crate::BoxedError;
use crate::args::Options;

const GRAY_DIFFUSION: [(u8, i8, i8, i8); 256] = {
    let mut table = [(0, 0, 0, 0); 256];
    let mut i = 0;

    while i < 256 {
        let gray = ((i + 8) / 17) * 17;
        let error = i as i16 - gray as i16;

        table[i] = (
            gray as u8,
            (error * 7 / 16) as i8,
            (error * 3 / 16) as i8,
            (error * 5 / 16) as i8,
        );

        i += 1;
    }

    table
};

fn dither_gray(page: &mut GrayImage) {
    let width = page.width() as usize;
    let mut rows = page.as_mut().chunks_exact_mut(width);
    let Some(mut row) = rows.next() else { return };

    for next in rows {
        let mut right = 0;
        let mut left_below: Option<&mut u8> = None;

        for (pixel, below) in row.iter_mut().zip(&mut *next) {
            let (gray, east, southwest, south) =
                GRAY_DIFFUSION[pixel.saturating_add_signed(right) as usize];
            *pixel = gray;
            right = east;

            if let Some(left) = left_below {
                *left = left.saturating_add_signed(southwest);
            }

            *below = below.saturating_add_signed(south);
            left_below = Some(below);
        }

        row = next;
    }

    let mut right = 0;

    for pixel in row {
        let (gray, east, ..) = GRAY_DIFFUSION[pixel.saturating_add_signed(right) as usize];
        *pixel = gray;
        right = east;
    }
}

pub struct DecodeWorker {
    resizer: fir::Resizer,
    options: Options,
}

impl DecodeWorker {
    pub fn new(options: Options) -> Self {
        Self {
            resizer: fir::Resizer::default(),
            options,
        }
    }

    fn encode_gray_png(page: &GrayImage) -> Result<Vec<u8>, BoxedError> {
        let palette: Vec<u8> = (0u8..16).flat_map(|level| [level * 17; 3]).collect();

        let width = page.width() as usize;
        let stride = width.div_ceil(2);
        let mut packed = vec![0; stride * page.height() as usize];
        for (row, dest) in page
            .as_raw()
            .chunks_exact(width)
            .zip(packed.chunks_exact_mut(stride))
        {
            // Dithering maps each pixel to 0x00, 0x11, ..., 0xff.
            let mut pairs = row.chunks_exact(2);
            for (byte, pair) in dest.iter_mut().zip(&mut pairs) {
                *byte = (pair[0] & 0xf0) | (pair[1] >> 4);
            }
            if let [last] = pairs.remainder() {
                // This also quantizes single-column pages, which skip dithering.
                dest[stride - 1] = (((u16::from(*last) + 8) / 17) as u8) << 4;
            }
        }

        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, page.width(), page.height());

        encoder.set_color(png::ColorType::Indexed);
        encoder.set_depth(png::BitDepth::Four);
        encoder.set_palette(palette);
        encoder.set_filter(png::Filter::NoFilter);
        encoder.set_deflate_compression(png::DeflateCompression::Level(3));

        let mut writer = encoder.write_header()?;

        writer.write_image_data(&packed)?;
        writer.finish()?;

        Ok(out)
    }

    fn resize(
        &mut self,
        img: DynamicImage,
        (max_w, max_h): (u32, u32),
    ) -> Result<DynamicImage, fir::ResizeError> {
        let src = match self.options.preserve_color {
            true => DynamicImage::ImageRgb8(img.into_rgb8()),
            false => DynamicImage::ImageLuma8(img.into_luma8()),
        };

        let (w, h) = src.dimensions();

        if w <= max_w && h <= max_h {
            return Ok(src);
        }

        let scale = (max_w as f64 / w as f64).min(max_h as f64 / h as f64);
        let scaled = |n: u32| ((n as f64 * scale).round() as u32).max(1);
        let mut dest = DynamicImage::new(scaled(w), scaled(h), src.color());

        self.resizer.resize(&src, &mut dest, None)?;
        Ok(dest)
    }

    fn encode(&mut self, img: DynamicImage, target: (u32, u32)) -> Result<Vec<u8>, BoxedError> {
        let mut page = self.resize(img, target)?;

        if let DynamicImage::ImageLuma8(gray) = &mut page {
            if gray.width() >= 2 && gray.height() >= 1 {
                dither_gray(gray);
            }

            return Self::encode_gray_png(gray);
        }

        let mut out: Vec<u8> = Vec::new();

        JpegEncoder::new_with_quality(&mut out, self.options.quality).encode(
            page.as_bytes(),
            page.width(),
            page.height(),
            page.color().into(),
        )?;

        Ok(out)
    }

    pub fn process_page(
        &mut self,
        img: DynamicImage,
        target: (u32, u32),
        median_portrait_ratio: Option<f64>,
    ) -> Result<Vec<Vec<u8>>, BoxedError> {
        let mut out = Vec::new();
        let ratio = img.width() as f64 / img.height() as f64;
        let is_spread = match median_portrait_ratio {
            Some(m) => ratio > 1.0 && (ratio / (2.0 * m) - 1.0).abs() <= 0.15,
            None => false,
        };

        let ss = self.options.split_spreads;
        let rs = self.options.rotate_spreads;
        let mut enc = |v| self.encode(v, target);

        if is_spread {
            let mut one_selected = false;

            if ss {
                let (width, height) = img.dimensions();
                let mid = width / 2;

                out.push(enc(img.crop_imm(mid, 0, width - mid, height))?);
                out.push(enc(img.crop_imm(0, 0, mid, height))?);
                one_selected = true;
            }

            if rs {
                out.push(enc(img.rotate90())?);
                one_selected = true;
            }

            if !one_selected {
                out.push(enc(img)?);
            }

            return Ok(out);
        } else {
            out.push(enc(img)?);
            return Ok(out);
        }
    }
}
