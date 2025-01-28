/// These tests compare results from this crate to similar operations performed with libsquashfs
/// from squashfs-tools-ng. libsquashfs APIs are accessed via the squashfs_ng crate.

use std::iter;
use anyhow;

use squashfs_tools::squashfs;
//use squashfs_tools::squashfs::metadata::{self, InodeType, EntryReference};
//use squashfs_tools::squashfs::superblock::Superblock;

use squashfs_ng::read;

const TEST_ARCHIVE: &str = "test_data/1.squashfs";

/// Check that the file_names read from the root directory are the same
#[test]
fn test_root() -> anyhow::Result<()> {
    let archive = read::Archive::new(TEST_ARCHIVE)?;
    let archive_rootdir = archive.get_exists("/")?.into_owned_dir()?;

    let mut sqfs = squashfs::SquashFS::open(TEST_ARCHIVE)?;
    let sqfs_rootdir = sqfs.read_dir("/")?;

    let z = iter::zip(archive_rootdir, sqfs_rootdir);
    for (ng, sq) in z {
        let ng = ng?;
        assert_eq!(ng.name().unwrap(), sq.file_name());
        println!("Match: {}", sq.file_name());
    }

    Ok(())
}

/// Check that the file_names read from the entire directory tree are the same
#[test]
fn test_tree() -> anyhow::Result<()> {
    let archive = read::Archive::new(TEST_ARCHIVE)?;
    let archive_rootnode = archive.get_exists("/")?;

    let mut sqfs = squashfs::SquashFS::open(TEST_ARCHIVE)?;
    let sqfs_rootnode = sqfs.root_inode()?;

    let total = compare_and_descend(&mut sqfs, &sqfs_rootnode, &archive, archive_rootnode)?;
    println!("Compared {} entries", total);

    Ok(())
}

fn compare_and_descend(
    sqfs: &mut squashfs::SquashFS<std::fs::File>, sq_inode: &squashfs::metadata::Inode,
    archive: &read::Archive, ng_inode: read::Node<'_>)
    -> anyhow::Result<u32>
{
    assert!(sq_inode.is_dir());
    assert!(ng_inode.is_dir()?);

    let archive_dir = ng_inode.into_owned_dir()?;
    let sqfs_dir = sqfs.read_dir_inode(sq_inode)?;

    let mut total = 0;
    let z = iter::zip(archive_dir, sqfs_dir);
    for (ng, sq) in z {
        let ng = ng?;
        let sq_inode = sqfs.inode(sq.inode_ref())?;
        // Compare the entry info
        assert_eq!(ng.name().unwrap(), sq.file_name());

        // Compare the inode info
        compare_inode(sqfs, &sq_inode, &ng)?;

        // If the inode represents a directory, recurse to compare the directory contents
        assert_eq!(sq_inode.is_dir(), ng.is_dir()?);
        if sq_inode.is_dir() {
            total += compare_and_descend(sqfs, &sq_inode, &archive, ng)?;
        }
        total += 1;
    }
    Ok(total)
}

fn compare_inode(sqfs: &mut squashfs::SquashFS<std::fs::File>,
    sq: &squashfs::metadata::Inode, ng: &read::Node<'_>) -> anyhow::Result<()>
{
    assert_eq!(sq.inode_number(), ng.id());
    assert_eq!(sq.mode(), ng.mode());
    assert_eq!(sq.uid(&sqfs.id_table)?, ng.uid()?);
    assert_eq!(sq.gid(&sqfs.id_table)?, ng.gid()?);
    assert_eq!(sq.mtime(), ng.mtime());
    // TODO: Extended attributes

    // TODO: File Data

    Ok(())
}