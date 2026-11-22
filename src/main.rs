mod args;

use std::error::Error;
use std::fs::File;
use std::io::BufWriter;
use std::io::IsTerminal;
use std::io::Read;
use std::io::Write;
use std::io::{self};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Instant;

use fast_image_resize as fir;

use flate2::read::DeflateDecoder;

use image::DynamicImage;
use image::GrayImage;
use image::codecs::jpeg::JpegEncoder;

use progress_bar::Color;
use progress_bar::Style;
use progress_bar::finalize_progress_bar;
use progress_bar::inc_progress_bar;
use progress_bar::init_progress_bar;
use progress_bar::set_progress_bar_action;
use rawzip::CompressionMethod;
use rawzip::FileReader;
use rawzip::ZipArchive;
use rawzip::ZipArchiveEntryWayfinder;
use rawzip::ZipArchiveWriter;
use rayon::iter::IntoParallelRefIterator;
use rayon::iter::ParallelIterator;

use crate::args::Args;
use crate::args::OutputStyle;

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
        size: (u32, u32),
    ) -> Result<(Vec<u8>, (u32, u32)), BoxErr> {
        let local = archive.get_entry(wayfinder)?;
        let decompressor = DeflateDecoder::new(local.reader());

        self.buf.clear();
        local
            .verifying_reader(decompressor)
            .read_to_end(&mut self.buf)?;

        let img = image::load_from_memory(&self.buf)?;
        let gray = self.prepare(img, size)?;

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

fn main() -> Result<(), BoxErr> {
    let args = Args::parse()?;
    let resolution = args.device.resolution();

    let file = File::open("examples/Dr. STONE v01.cbz")?;
    let mut archive_buf = vec![0u8; rawzip::RECOMMENDED_BUFFER_SIZE];
    let archive = ZipArchive::from_file(file, &mut archive_buf)?;

    let t0 = Instant::now();

    let pages = collect_pages(&archive, &mut archive_buf)?;
    let total = pages.len();

    if args.style == OutputStyle::Progress {
        init_progress_bar(total);
        set_progress_bar_action("Encoding", Color::Green, Style::Bold);
    }

    let i = AtomicUsize::new(1);
    let is_tty = io::stdout().is_terminal();

    let encoded: Vec<(&str, Vec<u8>, (u32, u32))> = pages
        .par_iter()
        .map_init(
            || Worker {
                buf: Vec::new(),
                resizer: fir::Resizer::new(),
            },
            |worker, (path, wayfinder)| {
                let (data, dimensions) = worker.process_page(&archive, *wayfinder, resolution)?;

                match args.style {
                    OutputStyle::Simple => {
                        let mut out = io::stdout().lock();
                        let n = i.fetch_add(1, Ordering::AcqRel);
                        if is_tty {
                            write!(out, "\r\x1b[2K[{}/{}]", n, total).ok();
                            out.flush().ok();
                        } else {
                            writeln!(out, "[{}/{}]", n, total).ok();
                        }
                    }
                    OutputStyle::Progress => inc_progress_bar(),
                    OutputStyle::JSON => {}
                }

                Ok::<_, BoxErr>((path.as_str(), data, dimensions))
            },
        )
        .collect::<Result<_, _>>()?;

    let output_file = File::create("examples/drstonesuperfigo.cbz")?;
    let mut output_archive = ZipArchiveWriter::new(BufWriter::new(output_file));

    for (path, data, (w, h)) in encoded.iter() {
        let (mut entry, config) = output_archive
            .new_file(path)
            .compression_method(CompressionMethod::STORE)
            .start()?;

        let mut writer = config.wrap(&mut entry);
        io::copy(&mut data.as_slice(), &mut writer)?;
        let (_, descriptor) = writer.finish()?;
        entry.finish(descriptor)?;
    }

    output_archive.finish()?;

    match args.style {
        OutputStyle::Simple => {
            if is_tty {
                println!()
            }
        }
        OutputStyle::Progress => {
            finalize_progress_bar();
        }
        _ => {}
    }

    println!("finished in {:?}", t0.elapsed());

    Ok(())
}
