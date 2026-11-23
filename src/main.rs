mod args;

use std::error::Error;
use std::fs;
use std::fs::File;
use std::io::BufWriter;
use std::io::IsTerminal;
use std::io::Read;
use std::io::Write;
use std::io::{self};
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
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
use rayon::prelude::*;

use crate::args::Args;

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

        let scale = (max_w as f64 / w as f64).min(max_h as f64 / h as f64);
        let scaled = |n: u32| ((n as f64 * scale).round() as u32).max(1);
        let mut dest = GrayImage::new(scaled(w), scaled(h));

        self.resizer.resize(&gray, &mut dest, None)?;
        Ok(dest)
    }

    fn process_page(
        &mut self,
        archive: &ZipArchive<FileReader>,
        wayfinder: ZipArchiveEntryWayfinder,
        resolution: (u32, u32),
    ) -> Result<Vec<u8>, BoxErr> {
        let local = archive.get_entry(wayfinder)?;
        let decompressor = DeflateDecoder::new(local.reader());

        self.buf.clear();
        local
            .verifying_reader(decompressor)
            .read_to_end(&mut self.buf)?;

        let img = image::load_from_memory(&self.buf)?;
        let gray = self.prepare(img, resolution)?;

        let mut out = Vec::new();
        JpegEncoder::new_with_quality(&mut out, 85).encode_image(&gray)?;
        Ok(out)
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

fn process_file(
    path: &Path,
    file_num: usize,
    file_total: usize,
    resolution: (u32, u32),
    is_tty: bool,
    archive_buf: &mut [u8],
) -> Result<(), BoxErr> {
    let file_started = Instant::now();

    let file = File::open(path)?;
    let archive = ZipArchive::from_file(file, archive_buf)?;
    let pages = collect_pages(&archive, archive_buf)?;

    let filename = path.file_name().unwrap_or(path.as_os_str());
    let label = if file_total > 1 {
        format!("[{file_num}/{file_total}] {}", filename.to_string_lossy())
    } else {
        filename.to_string_lossy().into_owned()
    };

    let page_total = pages.len();
    let completed = AtomicUsize::new(0);

    let encoded: Vec<(&str, Vec<u8>)> = pages
        .par_iter()
        .map_init(
            || Worker {
                buf: Vec::new(),
                resizer: fir::Resizer::new(),
            },
            |worker, (page_path, wayfinder)| {
                let data = worker.process_page(&archive, *wayfinder, resolution)?;
                let mut stdout = io::stdout().lock();
                let page_num = completed.fetch_add(1, Ordering::Relaxed) + 1;

                if is_tty {
                    write!(stdout, "\r\x1b[2K{label} [{page_num}/{page_total}]")?;
                    stdout.flush()?;
                } else if page_num < page_total {
                    writeln!(stdout, "{label} [{page_num}/{page_total}]")?;
                }

                Ok::<_, BoxErr>((page_path.as_str(), data))
            },
        )
        .collect::<Result<_, _>>()?;

    let output_path = path.with_extension("converted.cbz");
    let output_file = File::options()
        .write(true)
        .create_new(true)
        .open(&output_path)
        .map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("could not create {}: {err}", output_path.display()),
            )
        })?;

    let mut output = BufWriter::new(output_file);
    let mut output_archive = ZipArchiveWriter::new(&mut output);

    for (page_path, data) in encoded {
        let (mut entry, config) = output_archive
            .new_file(page_path)
            .compression_method(CompressionMethod::STORE)
            .start()?;

        let mut writer = config.wrap(&mut entry);
        writer.write_all(&data)?;
        let (_, descriptor) = writer.finish()?;
        entry.finish(descriptor)?;
    }

    output_archive.finish()?;
    output.flush()?;

    let seconds = file_started.elapsed().as_secs_f64();
    let mut stdout = io::stdout().lock();
    if is_tty {
        write!(stdout, "\r\x1b[2K")?;
    }
    writeln!(stdout, "{label} [{page_total}/{page_total}] {seconds:.2}s")?;

    Ok(())
}

fn prepare_path(path: PathBuf, recursive: bool, depth: usize) -> Result<Vec<PathBuf>, io::Error> {
    let mut out = Vec::new();

    if path.is_file()
        && path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("cbz"))
    {
        out.push(path);
        return Ok(out);
    }

    if path.is_dir() {
        if depth == 0 || recursive {
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                out.extend(prepare_path(entry.path(), recursive, depth + 1)?);
            }
        }
    }

    Ok(out)
}

fn main() -> Result<(), BoxErr> {
    let args = Args::parse()?;
    let resolution = args.device.resolution();
    let mut archive_buf = vec![0u8; rawzip::RECOMMENDED_BUFFER_SIZE];
    let is_tty = io::stdout().is_terminal();
    let file_total = args.paths.len();
    let started = Instant::now();
    let mut paths = Vec::new();

    for path in &args.paths {
        paths.extend(prepare_path(path.clone(), args.recursive, 0)?);
    }

    for (index, path) in paths.iter().enumerate() {
        process_file(
            path,
            index + 1,
            file_total,
            resolution,
            is_tty,
            &mut archive_buf,
        )?;
    }

    println!("finished in {:.2}s", started.elapsed().as_secs_f64());
    Ok(())
}
