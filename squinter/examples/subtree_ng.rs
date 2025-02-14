/// Dump contents of a SquashFS starting at a specified path
use std::env;

use anyhow;
use squashfs_ng::read::{self, Archive};

fn main() -> anyhow::Result<()> {
    let sqfs_path = env::args().nth(1).unwrap();
    let p = env::args().nth(2).unwrap();

    let sqfs = Archive::open(&sqfs_path)?;
    let i = sqfs.get_exists(p).unwrap();
    read_and_descend_ng(&sqfs, i, true)?;
    Ok(())
}

fn read_and_descend_ng(archive: &read::Archive, ng_inode: read::Node<'_>, content: bool)
    -> anyhow::Result<u32>
{
    assert!(ng_inode.is_dir()?);

    let archive_dir = ng_inode.into_owned_dir()?;

    let mut total = 0;
    for r in archive_dir {
        let node = r?;
        if content && node.is_file()? {
            let mut ng_reader = node.as_file()?;
            std::io::copy(&mut ng_reader, &mut std::io::sink())?;
        }
        // If the inode represents a directory, recurse to compare the directory contents
        if node.is_dir()? {
            total += read_and_descend_ng(&archive, node, content)?;
        }
        total += 1;
    }
    Ok(total)
}