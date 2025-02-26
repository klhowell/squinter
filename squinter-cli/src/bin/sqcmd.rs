use std::io::{Read, Seek, Write};
use std::path::PathBuf;

use anyhow::{self, Context};
use clap::{Args, Parser, Subcommand};
use squinter::squashfs::{Inode, SquashFS};
use squinter::squashfs::metadata::InodeExtendedInfo;
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
    #[arg(short)]
    long: bool,
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
                    files.push((file_arg.to_str().unwrap().to_string(), inode));
                }
            },
            Err(e) => {
                std::io::stderr().write_all(&format!("cannot access '{}': {}\n", file_arg.to_str().unwrap(), e).as_bytes())?;
            }
        }
    }
    if files.len() > 0 {
        if args.long {
            display_files_long(files)?;
        } else {
            display_files(files)?;
        }
        first = false;
    }
    // Next, print the contents of each directory argument, preceded by "<NAME>:"
    // If only a single path argument is supplied, do not precede with the "<NAME>:" header
    for file_arg in &args.files {
        match sqfs.inode_from_path(&file_arg) {
            Ok(inode) => {
                if inode.is_dir() {
                    let files: Vec<(String, Inode)> = sqfs.read_dir(&file_arg)?
                        .map(|de| (de.file_name(), sqfs.inode_from_entryref(de.inode_ref()).unwrap()))
                        .collect();
                    if !first { println!(""); }
                    if !single_path {
                        println!("{}:", file_arg.to_str().unwrap());
                    }
                    if args.long {
                        display_files_long(files)?;
                    } else {
                        display_files(files)?;
                    }
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

fn display_files(files: Vec<(String, Inode)>) -> anyhow::Result<()> {
    if files.is_empty() {
        return Ok(());
    }

    if termion::is_tty(&std::io::stdout()) {
        let (term_width, _) = termion::terminal_size()
            .context("Failed to read terminal size")?;
        let term_width: usize = term_width as usize;

        // The correct number of columns is somewhere between width/max_col_length and
        // width/min_col_length
        let lengths: Vec<usize> = files.iter().map(|(s,_)| s.len()).collect();
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
            for (n, (filename,_)) in files.iter().skip((row) as usize).step_by(files_per_column as usize).enumerate() {
                if n != 0 {
                    print!("   ");
                }
                print!("{:1$}", filename, col_widths[n] - 3);
            }
            println!();
        }
    } else {
        for (filename, _) in files {
            println!("{}", filename);
        }
    }
    Ok(())
}

fn display_files_long(files: Vec<(String, Inode)>) -> anyhow::Result<()> {
    if files.is_empty() {
        return Ok(());
    }

    for (filename, inode) in files {
        let link_postfix = match &inode.extended_info {
            InodeExtendedInfo::BasicSymlink(i) => {
                let mut s = String::from(" -> ");
                s.push_str(i.target_path.to_str()?);
                s
            }
            _ => { String::new() }
        };
        println!("{} {}{}", inode_mode_string(inode.mode()), filename, link_postfix);
    }
    Ok(())
}

fn inode_mode_string(mode: u16) -> String {
    let mut s = String::with_capacity(10);
    let mode_type = (mode & 0o170000) >> 12;
    let ch = match mode_type {
        1 => 'p',  // Pipe
        2 => 'c',  // char-dev
        4 => 'd',  // dir
        6 => 'b',  // block-dev
        8 => '-',  // file
        10 => 'l', // symlink
        12 => 's', // socket
        _ => '?',  // unknown
    };

    s.push(ch);
    s.push(if mode & 0o400 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o200 != 0 { 'w' } else { '-' });
    s.push(if mode & 0o100 != 0 { 'x' } else { '-' });
    s.push(if mode & 0o040 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o020 != 0 { 'w' } else { '-' });
    s.push(if mode & 0o010 != 0 { 'x' } else { '-' });
    s.push(if mode & 0o004 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o002 != 0 { 'w' } else { '-' });
    s.push(if mode & 0o001 != 0 { 'x' } else { '-' });
    s
}
