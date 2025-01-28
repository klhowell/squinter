use std::fs::File;
use std::path::{Path, PathBuf};

use clap::Parser;

use squashfs_tools::squashfs::{DirEntry, SquashFS};
use squashfs_tools::squashfs::metadata;

#[derive(Parser, Debug)]
struct Args {
    filesystem: PathBuf,
    files: Vec<PathBuf>,

    #[clap(short, action)]
    list_filesystem: bool,
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    if args.list_filesystem {
        list_filesystem(&args)?;
        Ok(())
    }
    else {
        eprintln!("Operation not implemented!");
        Err(std::io::Error::from(std::io::ErrorKind::Unsupported).into())
    }
}

fn list_filesystem(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let mut sqfs = SquashFS::open(&args.filesystem)?;

    let top_dir = PathBuf::from("squashfs-root");
    let file_list = if args.files.is_empty() {
        vec![top_dir.clone()]
    } else {
        args.files.iter()
            .map(|f| top_dir.join(f.strip_prefix("/").unwrap_or(f)))
            .collect()
    };

    let root_inode = sqfs.root_inode()?;
    println!("{}", top_dir.to_str().unwrap());
    for d in sqfs.read_dir_inode(&root_inode)? {
        print_and_descend_dir(&mut sqfs, &file_list, &top_dir, &d)?;
    }
    Ok(())
}

fn print_and_descend_dir(sqfs: &mut SquashFS<File>, files: &Vec<PathBuf>, parent: &Path, d: &DirEntry) -> Result<(), Box<dyn std::error::Error>> {
    let path = parent.join(d.file_name());

    if !files.iter().any(|p| path.starts_with(p) || p.starts_with(&path)) {
        return Ok(());
    }

    println!("{}", path.to_str().unwrap());

    let inode = sqfs.inode(d.inode_ref())?;
    if matches!(inode.inode_type, metadata::InodeType::BasicDir) {
        for d in sqfs.read_dir_inode(&inode)? {
            print_and_descend_dir(sqfs, files, &path, &d)?;
        }
    }
    Ok(())
}