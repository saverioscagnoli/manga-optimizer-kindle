use std::error::Error;
use std::fs::File;
use std::io::BufWriter;
use std::io::Read;
use std::io::{self};
use std::time::Instant;

use fast_image_resize as fir;

use flate2::read::DeflateDecoder;

use image::DynamicImage;
use image::GrayImage;
use image::codecs::jpeg::JpegEncoder;

use rawzip::CompressionMethod;
use rawzip::FileReader;
use rawzip::ZipArchive;
use rawzip::ZipArchiveEntryWayfinder;
use rawzip::ZipArchiveWriter;
use rayon::iter::IntoParallelRefIterator;
use rayon::iter::ParallelIterator;

type BoxErr = Box<dyn Error + Send + Sync>;

struct Worker {
    buf: Vec<u8>,
    resizer: fir::Resizer,
}

impl Worker {
    fn prepare(
        &mut self,
        img: DynamicImage,
        (max_w, max_h): (u32, u32),
    ) -> Result<GrayImage, fir::ResizeError> {
        let gray = img.into_luma8();
        let (w, h) = gray.dimensions();

        if w <= max_w && h <= max_h {
            return Ok(gray);
        }

        let max_w_f = max_w as f64;
        let max_h_f = max_h as f64;
        let w_f = w as f64;
        let h_f = h as f64;

        let scale = (max_w_f / w_f).min(max_h_f / h_f);
        let scaled = |n: u32| ((n as f64 * scale).round() as u32).max(1);

        let mut dest = GrayImage::new(scaled(w), scaled(h));

        self.resizer.resize(&gray, &mut dest, None)?;
        Ok(dest)
    }

    fn process_page(
        &mut self,
        archive: &ZipArchive<FileReader>,
        wayfinder: ZipArchiveEntryWayfinder,
    ) -> Result<(Vec<u8>, (u32, u32)), BoxErr> {
        let local = archive.get_entry(wayfinder)?;
        let decompressor = DeflateDecoder::new(local.reader());

        self.buf.clear();
        local
            .verifying_reader(decompressor)
            .read_to_end(&mut self.buf)?;

        let img = image::load_from_memory(&self.buf)?;
        let gray = self.prepare(img, OASIS)?;

        let mut out = Vec::new();
        JpegEncoder::new_with_quality(&mut out, 85).encode_image(&gray)?;

        Ok((out, gray.dimensions()))
    }
}

fn collect_pages(
    archive: &ZipArchive<FileReader>,
    archive_buf: &mut [u8],
) -> Result<Vec<(String, ZipArchiveEntryWayfinder)>, rawzip::Error> {
    let mut entries = archive.entries(archive_buf);
    let mut pages = Vec::new();

    while let Some(entry) = entries.next_entry()? {
        let path = entry.file_path().try_normalize()?.as_str().to_owned();
        pages.push((path, entry.wayfinder()));
    }

    pages.sort_by(|a, b| natord::compare(&a.0, &b.0));
    Ok(pages)
}

const OASIS: (u32, u32) = (1264, 1680);

fn main() -> Result<(), BoxErr> {
    let file = File::open("examples/Dr. STONE v01.cbz")?;
    let mut archive_buf = vec![0u8; rawzip::RECOMMENDED_BUFFER_SIZE];
    let archive = ZipArchive::from_file(file, &mut archive_buf)?;

    let t0 = Instant::now();

    let pages = collect_pages(&archive, &mut archive_buf)?;
    let total = pages.len();

    let encoded: Vec<(&str, Vec<u8>, (u32, u32))> = pages
        .par_iter()
        .map_init(
            || Worker {
                buf: Vec::new(),
                resizer: fir::Resizer::new(),
            },
            |w, (path, wayfinder)| {
                w.process_page(&archive, *wayfinder)
                    .map(|(data, dim)| (path.as_str(), data, dim))
            },
        )
        .collect::<Result<_, _>>()?;

    let output_file = File::create("examples/drstonesuperfigo.cbz")?;
    let mut output_archive = ZipArchiveWriter::new(BufWriter::new(output_file));

    for (i, (path, data, (w, h))) in encoded.iter().enumerate() {
        let (mut entry, config) = output_archive
            .new_file(path)
            .compression_method(CompressionMethod::STORE)
            .start()?;

        let mut writer = config.wrap(&mut entry);
        io::copy(&mut data.as_slice(), &mut writer)?;
        let (_, descriptor) = writer.finish()?;
        entry.finish(descriptor)?;

        println!("[{}/{}]: {}x{}", i + 1, total, w, h);
    }

    output_archive.finish()?;
    println!("took {:?}", t0.elapsed());

    Ok(())
}
