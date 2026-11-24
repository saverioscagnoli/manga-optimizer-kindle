mod args;
mod bar;
mod converter;
mod worker;

use std::error::Error;

use crate::args::Args;
use crate::converter::Converter;

type BoxedError = Box<dyn Error + Send + Sync>;

fn main() -> Result<(), BoxedError> {
    let args = Args::parse()?;
    let mut converter = Converter::new(args);

    converter.init()?;
    converter.start()?;

    Ok(())
}
