use std::path::PathBuf;
use std::str::FromStr;

const BIN: &str = env!("CARGO_BIN_NAME");

#[derive(Default)]
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub enum OutputStyle {
    #[default]
    Progress,
    JSON,
}

impl FromStr for OutputStyle {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "progress" => Ok(Self::Progress),
            "json" => Ok(Self::JSON),
            o => Err(format!("invalid output: {}", o)),
        }
    }
}

#[derive(Default)]
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub enum Device {
    KindleGen1,
    KindleGen2,
    KindleDXGen2,
    KindleDXGraphiteGen2,
    KindleKeyboardGen3,
    KindleGen4,
    KindleTouchGen4,
    KindleGen5,
    KindlePaperwhite1Gen5,
    KindlePaperwhite2Gen6,
    KindleGen7,
    KindleVoyageGen7,
    KindlePaperwhite3Gen7,
    KindleGen8,
    KindleOasis1Gen8,
    KindleOasis2Gen9,
    KindlePaperwhite4Gen10,
    KindleGen10,
    KindleOasis3Gen10,
    KindlePaperwhite5Gen11,
    KindleGen11,
    KindleScribeGen1,
    KindleGen11Release2024,
    #[default]
    KindlePaperwhite6Gen12,
    KindleColorsoftGen1,
    KindleScribeGen2,
}

struct Spec {
    device: Device,
    slug: &'static str,
    size: (u32, u32),
}

use Device::*;

#[rustfmt::skip]
const SPECS: &[Spec] = &[
    Spec { device: KindleGen1,              slug: "gen-1",               size: (600, 800)   },
    Spec { device: KindleGen2,              slug: "gen-2",               size: (600, 800)   },
    Spec { device: KindleDXGen2,            slug: "dx-gen-2",            size: (824, 1200)  },
    Spec { device: KindleDXGraphiteGen2,    slug: "dx-graphite-gen-2",   size: (824, 1200)  },
    Spec { device: KindleKeyboardGen3,      slug: "keyboard-gen-3",      size: (600, 800)   },
    Spec { device: KindleGen4,              slug: "gen-4",               size: (600, 800)   },
    Spec { device: KindleTouchGen4,         slug: "touch-gen-4",         size: (600, 800)   },
    Spec { device: KindleGen5,              slug: "gen-5",               size: (600, 800)   },
    Spec { device: KindlePaperwhite1Gen5,   slug: "paperwhite-1-gen-5",  size: (758, 1024)  },
    Spec { device: KindlePaperwhite2Gen6,   slug: "paperwhite-2-gen-6",  size: (758, 1024)  },
    Spec { device: KindleGen7,              slug: "gen-7",               size: (600, 800)   },
    Spec { device: KindleVoyageGen7,        slug: "voyage-gen-7",        size: (1072, 1448) },
    Spec { device: KindlePaperwhite3Gen7,   slug: "paperwhite-3-gen-7",  size: (1072, 1448) },
    Spec { device: KindleGen8,              slug: "gen-8",               size: (600, 800)   },
    Spec { device: KindleOasis1Gen8,        slug: "oasis-1-gen-8",       size: (1072, 1448) },
    Spec { device: KindleOasis2Gen9,        slug: "oasis-2-gen-9",       size: (1264, 1680) },
    Spec { device: KindlePaperwhite4Gen10,  slug: "paperwhite-4-gen-10", size: (1072, 1448) },
    Spec { device: KindleGen10,             slug: "gen-10",              size: (600, 800)   },
    Spec { device: KindleOasis3Gen10,       slug: "oasis-3-gen-10",      size: (1264, 1680) },
    Spec { device: KindlePaperwhite5Gen11,  slug: "paperwhite-5-gen-11", size: (1236, 1648) },
    Spec { device: KindleGen11,             slug: "gen-11",              size: (1072, 1448) },
    Spec { device: KindleScribeGen1,        slug: "scribe-gen-1",        size: (1860, 2480) },
    Spec { device: KindleGen11Release2024,  slug: "gen-11-2024",         size: (1072, 1448) },
    Spec { device: KindlePaperwhite6Gen12,  slug: "paperwhite-6-gen-12", size: (1272, 1696) },
    Spec { device: KindleColorsoftGen1,     slug: "colorsoft-gen-1",     size: (1272, 1696) },
    Spec { device: KindleScribeGen2,        slug: "scribe-gen-2",        size: (1860, 2480) },
];

impl Device {
    pub fn resolution(self) -> (u32, u32) {
        SPECS.iter().find(|s| s.device == self).unwrap().size
    }
}

impl FromStr for Device {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let norm = s.trim().to_ascii_lowercase().replace(['_', ' '], "-");
        let key = norm.strip_prefix("kindle-").unwrap_or(&norm);

        SPECS
            .iter()
            .find(|spec| spec.slug == key)
            .map(|spec| spec.device)
            .ok_or_else(|| {
                let valid = SPECS.iter().map(|s| s.slug).collect::<Vec<_>>();
                format!(
                    "unknown device `{s}`, expected one of: {}",
                    valid.join(", ")
                )
            })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub style: OutputStyle,
    pub device: Device,
    pub quality: u8,
    pub split_spreads: bool,
    pub rotate_spreads: bool,
    pub recursive: bool,
    pub preserve_color: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            style: OutputStyle::default(),
            device: Device::default(),
            quality: 85,
            split_spreads: false,
            rotate_spreads: false,
            recursive: false,
            preserve_color: false,
        }
    }
}

#[derive(Default)]
#[derive(Debug, Clone)]
pub struct Args {
    pub paths: Vec<PathBuf>,
    pub options: Options,
}

impl Args {
    fn print_usage() {
        println!("usage: {BIN} [OPTIONS] <PATH>...");
    }

    fn print_help() {
        Self::print_usage();
        println!();
        println!("optimizes .cbz file(s) for the selected ereader.");
        println!();
        println!("options:");
        println!("  -s, --style <STYLE>     progress display: simple, progress, json");
        println!("                          (default: simple)");
        println!("  -d, --device <DEVICE>   target device, will optimize for this");
        println!("                          (default: paperwhite-6-gen-12)");
        println!("  -D, --list-devices      list supported device models and their names");
        println!("  -q, --quality <QUALITY> JPEG quality when preserving color");
        println!("                          (default: 85) (1-100)");
        println!("  -S, --split-spreads     splits two-page spreads into two separate pages");
        println!("  -R, --rotate-spreads    rotates two-page spreads by 90 degrees");
        println!("  -r, --recursive         recursively process entries in directories");
        println!("  -C, --preserve-color    doesn't convert the files into grayscale");
        println!("  -h, --help              show this help message");
    }

    fn print_device_list() {
        for spec in SPECS {
            println!("  - {}", spec.slug);
        }

        println!("pass one of these to the `-d` or `--device` argument.");
        println!("the default one is `paperwhite-6-gen-12`");
    }

    pub fn parse() -> Result<Self, lexopt::Error> {
        use lexopt::prelude::*;

        let mut args = Self::default();
        let mut parser = lexopt::Parser::from_env();

        while let Some(arg) = parser.next()? {
            match arg {
                Short('h') | Long("help") => {
                    Self::print_help();
                    std::process::exit(0);
                }
                Value(path) => args.paths.push(path.parse()?),
                Short('s') | Long("style") => args.options.style = parser.value()?.parse()?,
                Short('d') | Long("device") => args.options.device = parser.value()?.parse()?,
                Short('q') | Long("quality") => args.options.quality = parser.value()?.parse()?,
                Short('S') | Long("split-spreads") => args.options.split_spreads = true,
                Short('R') | Long("rotate-spreads") => args.options.rotate_spreads = true,
                Short('r') | Long("recursive") => args.options.recursive = true,
                Short('C') | Long("preserve-color") => args.options.preserve_color = true,
                Short('D') | Long("list-devices") => {
                    Self::print_device_list();
                    std::process::exit(0);
                }
                _ => return Err(arg.unexpected()),
            }
        }

        if args.paths.is_empty() {
            println!("missing <path>, you must point to a file or to a directory.");
            println!("use {BIN} --help for additional info.");
            Self::print_usage();
            std::process::exit(1);
        }

        Ok(args)
    }
}
