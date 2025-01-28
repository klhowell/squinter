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
    let default_file_list = &vec![PathBuf::from("/")];
    let mut sqfs = SquashFS::open(&args.filesystem)?;

    let file_list = if args.files.is_empty() {
        &default_file_list
    } else {
        &args.files
    };

    for f in file_list {
        for d in sqfs.read_dir(f)? {
            print_and_descend_dir(&mut sqfs, f, &d)?;
        }
    }
    Ok(())
}

fn print_and_descend_dir(sqfs: &mut SquashFS<File>, parent: &Path, d: &DirEntry) -> Result<(), Box<dyn std::error::Error>> {
    let path = parent.join(d.file_name());
    println!("{}", path.to_str().unwrap());

    let inode = sqfs.inode(d.inode_ref())?;
    if matches!(inode.inode_type, metadata::InodeType::BasicDir) {
        for d in sqfs.read_dir_inode(&inode)? {
            print_and_descend_dir(sqfs, &path, &d)?;
        }
    }
    Ok(())
}