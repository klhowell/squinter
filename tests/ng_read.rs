use std::iter;
use anyhow;

use squashfs_tools::squashfs;
//use squashfs_tools::squashfs::metadata::{self, InodeType, EntryReference};
//use squashfs_tools::squashfs::superblock::Superblock;

use squashfs_ng::read;

const TEST_ARCHIVE: &str = "test_data/1.squashfs";

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