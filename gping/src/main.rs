use anyhow::Result;
use clap::{CommandFactory, Parser};
use std::path::Path;

mod app;
mod bigdigits;
mod cli;
mod color_depth;
mod format;
mod history;
mod hosts;
mod plot;
mod probes;
mod region_map;
mod run;
mod stats;
mod terminal;
mod theme;
mod ui;

fn generate_man_page(path: &Path) -> Result<()> {
    // The version is left out so the page doesn't change with every release. That also
    // drops clap's automatic -V/--version, so it's described by hand.
    let command = cli::Args::command().version(None).long_version(None).arg(
        clap::Arg::new("version")
            .short('V')
            .long("version")
            .action(clap::ArgAction::SetTrue)
            .help("Print version"),
    );
    let man = clap_mangen::Man::new(command);
    let mut buffer: Vec<u8> = Default::default();
    man.render(&mut buffer)?;

    std::fs::write(path, buffer)?;
    Ok(())
}

fn main() -> Result<()> {
    if let Some(path) = std::env::var_os("GENERATE_MANPAGE") {
        return generate_man_page(Path::new(&path));
    };
    let args = cli::Args::parse();
    run::finish(run::run(args))
}
