use std::io::{Read, Seek, Write};
use std::path::PathBuf;

use anyhow::{self, Context};
use clap::{Args, Parser, Subcommand};
use squinter::squashfs::SquashFS;
use termion;

#[derive(Parser, Debug)]
struct Cli {
    /// The SquashFS Filesystem to operate on
    filesystem: PathBuf,

    /// The command to execute
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Print file contents
    Cat(CatArgs),
    /// List files
    Ls(LsArgs),
}

#[derive(Args, Debug)]
struct CatArgs {
    files: Vec<PathBuf>,
}

#[derive(Args, Debug)]
struct LsArgs {
    files: Vec<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let mut sqfs = SquashFS::open(&cli.filesystem)
        .context("Failed to open SquashFS")?;
    match &cli.command {
        Command::Cat(args) => { cmd_cat(&mut sqfs, &cli, &args) },
        Command::Ls(args) => { cmd_ls(&mut sqfs, &cli, &args) }
    }
}

fn cmd_cat<R: Read+Seek>(sqfs: &mut SquashFS<R>, _cli: &Cli, args: &CatArgs) -> anyhow::Result<()> {
    for file_arg in &args.files {
        let inode = sqfs.inode_from_path(&file_arg)
            .context("Cannot open inode")?;
        if !inode.is_dir() {
            let mut reader = sqfs.open_file_inode(&inode)?;
            std::io::copy(&mut reader, &mut std::io::stdout())?;
        }
    }
    Ok(())
}

fn cmd_ls<R: Read+Seek>(sqfs: &mut SquashFS<R>, _cli: &Cli, args: &LsArgs) -> anyhow::Result<()> {
    let mut first = true;
    let single_path = args.files.len() == 1;

    // First, print non-directories that directly appeared as arguments
    let mut files = Vec::new();
    for file_arg in &args.files {
        match sqfs.inode_from_path(&file_arg) {
            Ok(inode) => {
                if !inode.is_dir() {
                    files.push(file_arg.to_str().unwrap().to_string());
                }
            },
            Err(e) => {
                std::io::stderr().write_all(&format!("cannot access '{}': {}\n", file_arg.to_str().unwrap(), e).as_bytes())?;
            }
        }
    }
    if files.len() > 0 {
        display_files(files)?;
        first = false;
    }
    // Next, print the contents of each directory argument, preceded by "<NAME>:"
    // If only a single path argument is supplied, do not precede with the "<NAME>:" header
    for file_arg in &args.files {
        match sqfs.inode_from_path(&file_arg) {
            Ok(inode) => {
                if inode.is_dir() {
                    let files: Vec<String> = sqfs.read_dir(&file_arg)?
                        .map(|de| de.file_name())
                        .collect();
                    if !first { println!(""); }
                    if !single_path {
                        println!("{}:", file_arg.to_str().unwrap());
                    }
                    display_files(files)?;
                    first = false;
                }
            },
            Err(e) => {
                std::io::stderr().write_all(format!("cannot access '{}': {}", file_arg.to_str().unwrap(), e).as_bytes())?;
            }
        }
    }
    Ok(())
}

fn display_files(files: Vec<String>) -> anyhow::Result<()> {
    if files.is_empty() {
        return Ok(());
    }

    //if termion::is_tty(&File::create("/dev/stdout")?) {
    if termion::is_tty(&std::io::stdout()) {
        let (term_width, _) = termion::terminal_size()
            .context("Failed to read terminal size")?;
        let term_width: usize = term_width as usize;

        // The correct number of columns is somewhere between width/max_col_length and
        // width/min_col_length
        let lengths: Vec<usize> = files.iter().map(|s| s.len()).collect();
        let min_columns = term_width / (*lengths.iter().max().unwrap() + 2);
        let max_columns = term_width / (*lengths.iter().min().unwrap() + 2);

        let mut columns = max_columns as usize;
        let mut col_widths: Vec<usize> = Vec::new();
        let mut files_per_column = 0;
        while columns >= min_columns {
            files_per_column = usize::div_ceil(lengths.len(), columns);
            // Figure out the width of each column
            col_widths = (0..columns).map(|c|
                lengths.iter()
                    .skip((files_per_column * c) as usize)
                    .take(files_per_column as usize)
                    .map(|l| *l + 3)
                    .max()
                    .unwrap_or(0)
                ).collect();
            if col_widths.iter().map(|l| *l).sum::<usize>() <= term_width {
                break;
            }
            columns -= 1;
        }

        for row in 0..files_per_column {
            for (n, filename) in files.iter().skip((row) as usize).step_by(files_per_column as usize).enumerate() {
                if n != 0 {
                    print!("   ");
                }
                print!("{:1$}", filename, col_widths[n] - 3);
            }
            println!();
        }
    } else {
        for filename in files {
            println!("{}", filename);
        }
    }
    Ok(())
}
