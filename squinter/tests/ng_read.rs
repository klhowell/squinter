/// These tests compare results from this crate to similar operations performed with libsquashfs
/// from squashfs-tools-ng. libsquashfs APIs are accessed via the squashfs_ng crate.
use std::time::Duration;
use std::io::{Read, Seek, BufReader};
use std::path::Path;
use std::iter;

use anyhow;
use test_assets_ureq::{TestAssetDef, dl_test_files_backoff};

use squinter::squashfs;
use squashfs_ng::read;

const TEST_DATA_DIR: &str = "../test_data";
const TEST_IMG_SRC: &str = "https://downloads.openwrt.org/releases/23.05.5/targets/layerscape/armv8_64b";
const TEST_IMG_NAME: &str = "openwrt-23.05.5-layerscape-armv8_64b-fsl_ls1012a-rdb-squashfs-firmware.bin";
const TEST_IMG_HASH: &str = "405331d0e203da3877f47934205e29a8835525e3deb1bd9c966e5102a05bc9a7";
const TEST_SQUASH_NAME: &str = "test.squashfs";
const TEST_SQUASH_OFFSET: u64 = 0x2000000;
const TEST_SQUASH_LEN: Option<u64> = None;

const COMPRESSION_METHODS: [&str;3] = ["gzip", "xz", "zstd"];

/// Check that the file_names read from the root directory are the same
#[cfg(feature = "flate2")]
#[test]
fn test_root_gzip() -> anyhow::Result<()> {
    test_root("gzip")
}

/// Check that the file_names read from the root directory are the same
#[cfg(feature = "lzma-rs")]
#[test]
fn test_root_xz() -> anyhow::Result<()> {
    test_root("xz")
}

/// Check that the file_names read from the root directory are the same
#[cfg(feature = "ruzstd")]
#[test]
fn test_root_zstd() -> anyhow::Result<()> {
    test_root("zstd")
}

fn test_root(c: &str) -> anyhow::Result<()> {
    prepare_test_files()?;

    let archive_path = format!("../test_data/test.{c}.squashfs");
    let archive = read::Archive::open(&archive_path)?;
    let archive_rootdir = archive.get_exists("/")?.into_owned_dir()?;

    let mut sqfs = squashfs::SquashFS::open(&archive_path)?;
    let sqfs_rootdir = sqfs.read_dir("/")?;

    let z = iter::zip(archive_rootdir, sqfs_rootdir);
    for (ng, sq) in z {
        let ng = ng?;
        assert_eq!(ng.name().unwrap(), sq.file_name());
        println!("Match: {}", sq.file_name());
    }

    Ok(())
}

/// Check that the file_names, attributes, and content read from the entire directory tree are the same
#[cfg(feature = "flate2")]
#[test]
fn test_tree_gzip() -> anyhow::Result<()> {
    test_tree("gzip")
}

/// Check that the file_names, attributes, and content read from the entire directory tree are the same
#[cfg(feature = "lzma-rs")]
#[test]
fn test_tree_xz() -> anyhow::Result<()> {
    test_tree("xz")
}

/// Check that the file_names, attributes, and content read from the entire directory tree are the same
#[cfg(feature = "ruzstd")]
#[test]
fn test_tree_zstd() -> anyhow::Result<()> {
    test_tree("zstd")
}

fn test_tree(c: &str) -> anyhow::Result<()> {
    prepare_test_files()?;

    let archive_path = format!("../test_data/test.{c}.squashfs");
    let archive = read::Archive::open(&archive_path)?;
    let archive_rootnode = archive.get_exists("/")?;

    let mut sqfs = squashfs::SquashFS::open(&archive_path)?;
    let sqfs_rootnode = sqfs.root_inode()?;

    let total = compare_and_descend(&mut sqfs, &sqfs_rootnode, &archive, archive_rootnode)?;
    println!("Compared {} entries", total);

    Ok(())
}

fn compare_and_descend(
    sqfs: &mut squashfs::SquashFS<BufReader<std::fs::File>>, sq_inode: &squashfs::metadata::Inode,
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
        let sq_inode = sqfs.inode_from_entryref(sq.inode_ref())?;
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

fn compare_inode(sqfs: &mut squashfs::SquashFS<BufReader<std::fs::File>>,
    sq: &squashfs::metadata::Inode, ng: &read::Node<'_>) -> anyhow::Result<()>
{
    assert_eq!(sq.inode_number(), ng.id());
    assert_eq!(sq.mode(), ng.mode());
    assert_eq!(sq.uid(&sqfs)?, ng.uid()?);
    assert_eq!(sq.gid(&sqfs)?, ng.gid()?);
    assert_eq!(sq.mtime(), ng.mtime());
    // TODO: Extended attributes

    assert_eq!(sq.is_file(), ng.is_file()?);
    if sq.is_file() {
        let sq_reader = BufReader::new(sqfs.open_file_inode(sq)?);
        let ng_reader = BufReader::new(ng.as_file()?);
        assert!(ng_reader.bytes().map(|r| r.unwrap()).eq(sq_reader.bytes().map(|r| r.unwrap())));
    }

    Ok(())
}

fn prepare_test_files() -> std::io::Result<()> {
    // Get a publicly available SquashFS to test
    let test_asset_defs = [
        TestAssetDef {
            filename: TEST_IMG_NAME.to_string(),
            hash: TEST_IMG_HASH.to_string(),
            url: format!("{TEST_IMG_SRC}/{TEST_IMG_NAME}"),
        },
    ];
    let img_file = format!("{TEST_DATA_DIR}/{TEST_IMG_NAME}");
    dl_test_files_backoff(&test_asset_defs, TEST_DATA_DIR, true, Duration::from_secs(10)).unwrap();

    let test_file = format!("{TEST_DATA_DIR}/{TEST_SQUASH_NAME}");
    if !Path::new(&test_file).exists() {
        extract_squash(&img_file, &test_file, TEST_SQUASH_OFFSET, TEST_SQUASH_LEN)?;
    }

    for c in COMPRESSION_METHODS {
        let comp_file = format!("{TEST_DATA_DIR}/test.{c}.squashfs");
        if !Path::new(&comp_file).exists() {
            recompress_squash(&test_file, &comp_file, c)?;
        }
    }

    Ok(())
}

fn extract_squash(in_file: &str, out_file: &str, start: u64, len: Option<u64>) -> std::io::Result<()> {
    let mut inf = std::fs::File::open(in_file)?;
    let mut outf = std::fs::File::create(out_file)?;
    inf.seek(std::io::SeekFrom::Start(start))?;

    if let Some(l) = len {
        let mut part = inf.take(l);
        std::io::copy(&mut part, &mut outf)?;
    } else {
        std::io::copy(&mut inf, &mut outf)?;
    }
    Ok(())
}

fn recompress_squash(in_file: &str, out_file: &str, comp: &str) -> std::io::Result<()> {
    let cmd = format!("sqfs2tar {in_file} | tar2sqfs -c {comp} {out_file}");
    std::process::Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .status()?;
    Ok(())
}
