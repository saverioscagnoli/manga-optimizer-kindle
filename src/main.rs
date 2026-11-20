use std::{
    error::Error,
    fs::File,
    io::{self, BufWriter, Read},
    time::Instant,
};

use fast_image_resize as fir;
use flate2::read::DeflateDecoder;
use image::{DynamicImage, GrayImage, codecs::jpeg::JpegEncoder};
use rawzip::{
    CompressionMethod, FileReader, ZipArchive, ZipArchiveEntryWayfinder, ZipArchiveWriter,
};

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

fn prepare_page(
    img: DynamicImage,
    (max_w, max_h): (u32, u32),
    resizer: &mut fir::Resizer,
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

    resizer.resize(&gray, &mut dest, None)?;
    Ok(dest)
}

const OASIS: (u32, u32) = (1264, 1680);

fn main() -> Result<(), Box<dyn Error>> {
    let file = File::open("examples/Dr. STONE v01.cbz")?;
    let mut archive_buf = vec![0u8; rawzip::RECOMMENDED_BUFFER_SIZE];
    let archive = ZipArchive::from_file(file, &mut archive_buf)?;
    let mut buf = Vec::new();
    let mut resizer = fir::Resizer::new();

    let output_file = File::create("examples/drstonesuperfigo.cbz")?;
    let mut output_archive = ZipArchiveWriter::new(BufWriter::new(output_file));
    let mut encode_buf = Vec::new();

    let t0 = Instant::now();

    let pages = collect_pages(&archive, &mut archive_buf)?;
    let total = pages.len();

    for (i, (path, wayfinder)) in pages.iter().enumerate() {
        let local = archive.get_entry(*wayfinder)?;
        let decompressor = DeflateDecoder::new(local.reader());

        buf.clear();
        local.verifying_reader(decompressor).read_to_end(&mut buf)?;

        let img = image::load_from_memory(&buf)?;
        let gray = prepare_page(img, OASIS, &mut resizer)?;

        encode_buf.clear();
        JpegEncoder::new_with_quality(&mut encode_buf, 85).encode_image(&gray)?;

        let (mut output_entry, config) = output_archive
            .new_file(path)
            .compression_method(CompressionMethod::STORE)
            .start()?;

        let mut writer = config.wrap(&mut output_entry);
        io::copy(&mut encode_buf.as_slice(), &mut writer)?;

        let (_, descriptor) = writer.finish()?;
        output_entry.finish(descriptor)?;

        println!("[{}/{}]: {}x{}", i + 1, total, gray.width(), gray.height())
    }

    output_archive.finish()?;
    println!("took {:?}", t0.elapsed());

    Ok(())
}
