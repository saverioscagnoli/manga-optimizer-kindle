use fast_image_resize as fir;
use image::DynamicImage;
use image::GenericImageView;
use image::GrayImage;
use image::Luma;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::ColorMap;
use image::imageops::dither;

use crate::BoxedError;

struct Gray16;

impl ColorMap for Gray16 {
    type Color = Luma<u8>;

    fn index_of(&self, color: &Self::Color) -> usize {
        (usize::from(color[0]) + 8) / 17
    }

    fn map_color(&self, color: &mut Self::Color) {
        color[0] = (self.index_of(color) * 17) as u8
    }
}

#[derive(Default)]
pub struct DecodeWorker {
    buffer: Vec<u8>,
    resizer: fir::Resizer,
}

impl DecodeWorker {
    fn encode_gray_png(page: &GrayImage) -> Result<Vec<u8>, BoxedError> {
        let palette: Vec<u8> = (0u8..16).flat_map(|level| [level * 17; 3]).collect();

        let width = page.width() as usize;
        let mut packed = Vec::with_capacity(width.div_ceil(2) * page.height() as usize);
        let index = |gray: u8| ((u16::from(gray) + 8) / 17) as u8;

        for row in page.as_raw().chunks_exact(width) {
            for pair in row.chunks(2) {
                let high = index(pair[0]);
                let low = pair.get(1).copied().map_or(0, index);
                packed.push((high << 4) | low);
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
        preserve_color: bool,
    ) -> Result<DynamicImage, fir::ResizeError> {
        let src = match preserve_color {
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

    fn encode(
        &mut self,
        img: DynamicImage,
        target: (u32, u32),
        quality: u8,
        preserve_color: bool,
    ) -> Result<Vec<u8>, BoxedError> {
        let mut page = self.resize(img, target, preserve_color)?;

        if let DynamicImage::ImageLuma8(gray) = &mut page {
            if gray.width() >= 2 && gray.height() >= 1 {
                dither(gray, &Gray16);
            }

            return Self::encode_gray_png(gray);
        }

        let mut out: Vec<u8> = Vec::new();

        JpegEncoder::new_with_quality(&mut out, quality).encode(
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
        quality: u8,
        preserve_color: bool,
    ) -> Result<Vec<Vec<u8>>, BoxedError> {
        let ratio = img.width() as f64 / img.height() as f64;
        let is_spread = match median_portrait_ratio {
            Some(m) => ratio > 1.0 && (ratio / (2.0 * m) - 1.0).abs() <= 0.15,
            None => false,
        };

        let mut enc = |v| self.encode(v, target, quality, preserve_color);

        if is_spread {
            let (width, height) = img.dimensions();
            let mid = width / 2;

            return Ok(vec![
                enc(img.crop_imm(mid, 0, width - mid, height))?,
                enc(img.crop_imm(0, 0, mid, height))?,
                enc(img.rotate90())?,
            ]);
        }

        Ok(vec![enc(img)?])
    }
}
