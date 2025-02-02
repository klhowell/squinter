use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use clap::Parser;
use squinter::squashfs::{DirEntry, SquashFS};

#[derive(Parser, Debug)]
struct Args {
    filesystem: PathBuf,
    files: Vec<PathBuf>,

    #[clap(short, long="dest", default_value="squashfs-root")]
    dir: PathBuf,

    #[clap(short, action)]
    list_filesystem: bool,

    #[clap(long="cat", action)]
    cat_files: bool,
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    if args.list_filesystem {
        list_filesystem(&args)?;
        Ok(())
    }
    else if args.cat_files {
        cat_files(&args)?;
        Ok(())
    }
    else {
        eprintln!("Operation not implemented!");
        Err(std::io::Error::from(std::io::ErrorKind::Unsupported).into())
    }
}

fn list_filesystem(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let mut sqfs = SquashFS::open(&args.filesystem)?;

    let file_list = if args.files.is_empty() {
        vec![args.dir.clone()]
    } else {
        args.files.iter()
            .map(|f| args.dir.join(f.strip_prefix("/").unwrap_or(f)))
            .collect()
    };

    let root_inode = sqfs.root_inode()?;
    println!("{}", args.dir.to_str().unwrap());
    for d in sqfs.read_dir_inode(&root_inode)? {
        print_and_descend_dir(&mut sqfs, &file_list, &args.dir, &d)?;
    }
    Ok(())
}

fn print_and_descend_dir(sqfs: &mut SquashFS<File>, files: &Vec<PathBuf>, parent: &Path, d: &DirEntry) -> Result<(), Box<dyn std::error::Error>> {
    let path = parent.join(d.file_name());

    if !files.iter().any(|p| path.starts_with(p) || p.starts_with(&path)) {
        return Ok(());
    }

    println!("{}", path.to_str().unwrap());

    let inode = sqfs.inode_from_entryref(d.inode_ref())?;
    if inode.is_dir() {
        for d in sqfs.read_dir_inode(&inode)? {
            print_and_descend_dir(sqfs, files, &path, &d)?;
        }
    }
    Ok(())
}

fn cat_files(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let mut sqfs = SquashFS::open(&args.filesystem)?;

    let file_list = if args.files.is_empty() {
        vec![args.dir.clone()]
    } else {
        args.files.iter()
            .map(|f| args.dir.join(f.strip_prefix("/").unwrap_or(f)))
            .collect()
    };

    let root_inode = sqfs.root_inode()?;
    for d in sqfs.read_dir_inode(&root_inode)? {
        cat_and_descend_dir(&mut sqfs, &file_list, &args.dir, &d)?;
    }
    Ok(())
}

fn cat_and_descend_dir(sqfs: &mut SquashFS<File>, files: &Vec<PathBuf>, parent: &Path, d: &DirEntry) -> Result<(), Box<dyn std::error::Error>> {
    let path = parent.join(d.file_name());

    if !files.iter().any(|p| path.starts_with(p) || p.starts_with(&path)) {
        return Ok(());
    }

    let inode = sqfs.inode_from_entryref(d.inode_ref())?;
    if inode.is_file() {
        //eprintln!("File -> {}", path.to_str().unwrap());
        let mut r = sqfs.open_file_inode(&inode)?;
        let stdout = io::stdout();
        let mut stdout = stdout.lock();
        io::copy(&mut r, &mut stdout)?;
    }
    if inode.is_dir() {
        //eprintln!("Dir -> {}", path.to_str().unwrap());
        for d in sqfs.read_dir_inode(&inode)? {
            cat_and_descend_dir(sqfs, files, &path, &d)?;
        }
    }
    Ok(())
}
