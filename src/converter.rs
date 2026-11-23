use std::fs::File;
use std::fs::{self};
use std::io::IsTerminal;
use std::io::Read;
use std::io::Write;
use std::io::{self};
use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;

use flate2::read::DeflateDecoder;
use image::DynamicImage;
use image::GenericImageView;
use rawzip::CompressionMethod;
use rawzip::FileReader;
use rawzip::ZipArchive;
use rawzip::ZipArchiveEntryWayfinder;
use rawzip::ZipArchiveWriter;
use rayon::iter::IndexedParallelIterator;
use rayon::iter::IntoParallelIterator;
use rayon::iter::IntoParallelRefIterator;
use rayon::iter::ParallelIterator;

use crate::BoxedError;
use crate::args::Args;
use crate::worker::DecodeWorker;

pub struct Converter {
    // State
    archive_buffer: Vec<u8>,
    paths: Vec<PathBuf>,

    // Computed
    resolution: (u32, u32),
    is_tty: bool,
    total: usize,

    // Options
    quality: u8,
    recursive: bool,
    preserve_color: bool,
}

impl Converter {
    pub fn new(args: Args) -> Self {
        let total = args.paths.len();

        Self {
            archive_buffer: vec![0; rawzip::RECOMMENDED_BUFFER_SIZE],
            paths: args.paths,
            resolution: args.device.resolution(),
            is_tty: io::stdout().is_terminal(),
            total,
            quality: args.quality,
            recursive: args.recursive,
            preserve_color: args.preserve_color,
        }
    }

    fn collect_paths(&self, out: &mut Vec<PathBuf>, path: PathBuf, depth: usize) -> io::Result<()> {
        let ext = path.extension();

        if path.is_file() && ext.is_some_and(|e| e.eq_ignore_ascii_case("cbz")) {
            out.push(path);
            return Ok(());
        }

        if path.is_dir() && (depth == 0 || self.recursive) {
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

    fn collect_pages(
        &mut self,
        archive: &ZipArchive<FileReader>,
    ) -> Result<Vec<(String, ZipArchiveEntryWayfinder)>, BoxedError> {
        let mut entries = archive.entries(&mut self.archive_buffer);
        let mut pages = Vec::new();

        while let Some(entry) = entries.next_entry()? {
            let path = entry.file_path().try_normalize()?.as_str().to_owned();
            pages.push((path, entry.wayfinder()));
        }

        pages.sort_by(|a, b| natord::compare(&a.0, &b.0));

        Ok(pages)
    }

    /// Decodes images in parallel and spits out the median portrait ratio,
    /// used for recognizing page spreads
    fn collect_images(
        archive: &ZipArchive<FileReader>,
        pages: &[(String, ZipArchiveEntryWayfinder)],
    ) -> Result<(Vec<DynamicImage>, Option<f64>), BoxedError> {
        let (images, sizes) = pages
            .par_iter()
            .map_init(Vec::<u8>::new, |buffer, (_, wayfinder)| {
                buffer.clear();

                let entry = archive.get_entry(*wayfinder)?;
                let decoder = DeflateDecoder::new(entry.reader());
                entry.verifying_reader(decoder).read_to_end(buffer)?;

                let img = image::load_from_memory(buffer)?;
                let size = img.dimensions();
                Ok((img, size))
            })
            .collect::<Result<(Vec<_>, Vec<_>), BoxedError>>()?;

        let ratio = Self::median_portrait_ratio(&sizes);
        Ok((images, ratio))
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
        let number_pages = output_count > self.total
            || data.iter().any(|(path, _)| {
                !Path::new(path)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("jpg"))
            });

        let digits = output_count.to_string().len();
        let output_pages = data
            .into_iter()
            .flat_map(|(path, views)| views.into_iter().map(move |view| (path.clone(), view)));

        for (index, (source_path, data)) in output_pages.enumerate() {
            let page_path = match number_pages {
                true => format!("{:0digits$}.jpg", index + 1),
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
        let (images, median_ratio) = Self::collect_images(&archive, &pages)?;

        let encoded: Vec<(String, Vec<Vec<u8>>)> = pages
            .par_iter()
            .zip(images.into_par_iter())
            .map_init(DecodeWorker::default, |worker, ((page_path, _), img)| {
                let data = worker.process_page(
                    img,
                    self.resolution,
                    median_ratio,
                    self.quality,
                    self.preserve_color,
                )?;
                Ok((page_path.clone(), data))
            })
            .collect::<Result<_, BoxedError>>()?;

        Ok((encoded, t0))
    }

    pub fn start(&mut self) -> Result<(), BoxedError> {
        let start = Instant::now();

        for p in self.paths.clone() {
            let (data, time) = self.process_file(&p)?;

            self.write_archive(&p, data)?;
        }

        println!("processed {} entries in {:?}", self.total, start.elapsed());

        Ok(())
    }
}
