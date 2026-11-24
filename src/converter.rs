use std::fs::File;
use std::fs::{self};
use std::io::IsTerminal;
use std::io::Read;
use std::io::Write;
use std::io::{self};
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Instant;

use flate2::read::DeflateDecoder;
use image::ImageReader;
use rawzip::CompressionMethod;
use rawzip::FileReader;
use rawzip::ZipArchive;
use rawzip::ZipArchiveEntryWayfinder;
use rawzip::ZipArchiveWriter;
use rayon::iter::IntoParallelRefIterator;
use rayon::iter::ParallelIterator;

use crate::BoxedError;
use crate::args::Args;
use crate::args::Options;
use crate::args::OutputStyle;
use crate::output::draw_bar;
use crate::output::json_string;
use crate::worker::DecodeWorker;

struct Page {
    path: String,
    wayfinder: ZipArchiveEntryWayfinder,
    compression: CompressionMethod,
}

impl Page {
    fn reader<'a>(
        &self,
        archive: &'a ZipArchive<FileReader>,
    ) -> Result<impl Read + 'a, BoxedError> {
        let entry = archive.get_entry(self.wayfinder)?;
        let raw = entry.reader();
        let decoded: Box<dyn Read + 'a> = match self.compression {
            CompressionMethod::STORE => Box::new(raw),
            CompressionMethod::DEFLATE => Box::new(DeflateDecoder::new(raw)),
            method => {
                return Err(
                    format!("unsupported ZIP compression {method} for {}", self.path).into(),
                );
            }
        };
        Ok(entry.verifying_reader(decoded))
    }
}

pub struct Converter {
    // State
    archive_buffer: Vec<u8>,
    paths: Vec<PathBuf>,
    options: Options,

    // Computed
    resolution: (u32, u32),
    is_tty: bool,
    total: usize,
}

impl Converter {
    pub fn new(args: Args) -> Self {
        let total = args.paths.len();

        Self {
            archive_buffer: vec![0; rawzip::RECOMMENDED_BUFFER_SIZE],
            paths: args.paths,
            resolution: args.options.device.resolution(),
            is_tty: io::stdout().is_terminal(),
            total,
            options: args.options,
        }
    }

    fn collect_paths(&self, out: &mut Vec<PathBuf>, path: PathBuf, depth: usize) -> io::Result<()> {
        let ext = path.extension();

        if path.is_file() && ext.is_some_and(|e| e.eq_ignore_ascii_case("cbz")) {
            out.push(path);
            return Ok(());
        }

        if path.is_dir() && (depth == 0 || self.options.recursive) {
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                self.collect_paths(out, entry.path(), depth + 1)?;
            }
        }

        Ok(())
    }

    pub fn init(&mut self) -> io::Result<()> {
        let mut computed: Vec<PathBuf> = Vec::new();

        self.paths
            .iter()
            .try_for_each(|p| self.collect_paths(&mut computed, p.clone(), 0))?;

        self.paths = computed;
        self.total = self.paths.len();

        Ok(())
    }

    fn median_portrait_ratio(sizes: &[(u32, u32)]) -> Option<f64> {
        let mut ratios = sizes
            .iter()
            .filter_map(|&(w, h)| (w > 0 && w < h).then(|| w as f64 / h as f64))
            .collect::<Vec<f64>>();

        if ratios.is_empty() {
            return None;
        }

        ratios.sort_unstable_by(f64::total_cmp);

        let len = ratios.len();
        let mid = len / 2;

        if len % 2 == 0 {
            Some((ratios[mid - 1] + ratios[mid]) / 2.0)
        } else {
            Some(ratios[mid])
        }
    }

    fn collect_pages(&mut self, archive: &ZipArchive<FileReader>) -> Result<Vec<Page>, BoxedError> {
        let mut entries = archive.entries(&mut self.archive_buffer);
        let mut pages = Vec::new();

        while let Some(entry) = entries.next_entry()? {
            if entry.is_dir() {
                continue;
            }

            let path = entry.file_path().try_normalize()?.as_str().to_owned();

            if !image::ImageFormat::from_path(&path).is_ok_and(|format| format.reading_enabled()) {
                continue;
            }

            pages.push(Page {
                path,
                wayfinder: entry.wayfinder(),
                compression: entry.compression_method(),
            });
        }

        pages.sort_by(|a, b| natord::compare(&a.path, &b.path));

        Ok(pages)
    }

    fn write_archive(
        &mut self,
        input_path: &Path,
        data: Vec<(String, Vec<Vec<u8>>)>,
    ) -> Result<(), BoxedError> {
        let output_path = input_path.with_added_extension("converted.cbz");
        let output_file = File::options()
            .write(true)
            .create_new(true)
            .open(&output_path)?;

        let mut output = io::BufWriter::new(output_file);
        let mut output_archive = ZipArchiveWriter::new(&mut output);

        let output_count = data.iter().map(|(_, views)| views.len()).sum::<usize>();
        let ext = if self.options.preserve_color {
            "jpg"
        } else {
            "png"
        };
        let number_pages = output_count > self.total
            || data.iter().any(|(path, _)| {
                !Path::new(path)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case(ext))
            });

        let digits = output_count.to_string().len();
        let output_pages = data
            .into_iter()
            .flat_map(|(path, views)| views.into_iter().map(move |view| (path.clone(), view)));

        for (index, (source_path, data)) in output_pages.enumerate() {
            let page_path = match number_pages {
                true => format!("{:0digits$}.{ext}", index + 1),
                false => source_path,
            };

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

        Ok(())
    }

    fn process_file(
        &mut self,
        path: &Path,
    ) -> Result<(Vec<(String, Vec<Vec<u8>>)>, Instant), BoxedError> {
        let t0 = Instant::now();
        let file = File::open(path)?;
        let archive = ZipArchive::from_file(file, &mut self.archive_buffer)?;
        let pages = self.collect_pages(&archive)?;
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy();

        let count = AtomicUsize::new(1);
        let total = pages.len();

        let sizes = pages
            .par_iter()
            .map_init(Vec::new, |buf, page| {
                let mut reader = page.reader(&archive)?;

                buf.clear();

                (&mut reader).take(8192).read_to_end(buf)?;
                let dimensions = |bytes: &[u8]| -> Result<_, BoxedError> {
                    Ok(ImageReader::new(io::Cursor::new(bytes))
                        .with_guessed_format()?
                        .into_dimensions()?)
                };

                if let Ok(size) = dimensions(buf) {
                    return Ok(size);
                }

                // Some image headers extend beyond the initial prefix
                reader.read_to_end(buf)?;
                dimensions(buf)
            })
            .collect::<Result<Vec<_>, BoxedError>>()?;

        let median_ratio = Self::median_portrait_ratio(&sizes);

        let encoded = pages
            .par_iter()
            .map_init(
                || (DecodeWorker::new(self.options), Vec::new()),
                |(worker, buf), page| {
                    buf.clear();
                    page.reader(&archive)?.read_to_end(buf)?;
                    let img = image::load_from_memory(buf)?;
                    let data = worker.process_page(img, self.resolution, median_ratio)?;

                    let done = count.fetch_add(1, Ordering::AcqRel);

                    match self.options.style {
                        OutputStyle::Progress => {
                            if self.is_tty {
                                draw_bar(done, total, &name)
                            }
                        }
                        OutputStyle::JSON => {
                            let path = json_string(&format!("{}", path.display()));
                            println!(r#"{{"input": {path}, "page": {done}, "total": {total}}}"#);
                        }
                    };

                    Ok((page.path.clone(), data))
                },
            )
            .collect::<Result<Vec<_>, BoxedError>>()?;

        Ok((encoded, t0))
    }

    pub fn start(&mut self) -> Result<(), BoxedError> {
        let started = Instant::now();

        for (i, p) in self.paths.clone().iter().enumerate() {
            let n = i + 1;
            let (data, started1) = self.process_file(&p)?;

            self.write_archive(&p, data)?;

            match self.options.style {
                OutputStyle::Progress => {
                    eprintln!("  {:.2?}", started1.elapsed())
                }
                OutputStyle::JSON => println!(
                    r#"{{"books_processed": {n}, "elapsed": "{:?}"}}"#,
                    started.elapsed()
                ),
            }
        }

        println!(
            "processed {} entries in {:.2?}",
            self.total,
            started.elapsed()
        );

        Ok(())
    }
}
