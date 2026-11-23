use fast_image_resize as fir;
use image::DynamicImage;
use image::GenericImageView;
use image::codecs::jpeg::JpegEncoder;

use crate::BoxedError;

#[derive(Default)]
pub struct DecodeWorker {
    buffer: Vec<u8>,
    resizer: fir::Resizer,
}

impl DecodeWorker {
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
        let page = self.resize(img, target, preserve_color)?;
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
