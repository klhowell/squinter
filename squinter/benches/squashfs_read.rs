
use anyhow;
use criterion::{criterion_group, criterion_main, Criterion};

use squashfs_ng::read;
use squashfs_tools::squashfs;
const TEST_ARCHIVE: &str = "test_data/1.squashfs";

fn read_root_sqfs() -> anyhow::Result<usize> {
    let mut sqfs = squashfs::SquashFS::open(TEST_ARCHIVE)?;
    let root_count = sqfs.read_dir("/")?
        .count();

    Ok(root_count)
}

fn read_root_ng() -> anyhow::Result<usize> {
    let archive = read::Archive::new(TEST_ARCHIVE)?;
    let root_count = archive.get_exists("/")?.into_owned_dir()?
        .count();

    Ok(root_count)
}

fn read_tree_sqfs() -> anyhow::Result<u32> {
    let mut sqfs = squashfs::SquashFS::open(TEST_ARCHIVE)?;
    let sqfs_rootnode = sqfs.root_inode()?;
    let total = read_and_descend_sqfs(&mut sqfs, &sqfs_rootnode)?;
    Ok(total)
}

fn read_tree_ng() -> anyhow::Result<u32> {
    let archive = read::Archive::new(TEST_ARCHIVE)?;
    let archive_rootnode = archive.get_exists("/")?;
    let total = read_and_descend_ng(&archive, archive_rootnode)?;
    Ok(total)
}

fn read_and_descend_sqfs(sqfs: &mut squashfs::SquashFS<std::fs::File>, sq_inode: &squashfs::metadata::Inode)
    -> anyhow::Result<u32>
{
    assert!(sq_inode.is_dir());

    let sqfs_dir = sqfs.read_dir_inode(sq_inode)?;

    let mut total = 0;
    for de in sqfs_dir {
        let sq_inode = sqfs.inode(de.inode_ref())?;
        // If the inode represents a directory, recurse to the directory contents
        if sq_inode.is_dir() {
            total += read_and_descend_sqfs(sqfs, &sq_inode)?;
        }
        total += 1;
    }
    Ok(total)
}

fn read_and_descend_ng(archive: &read::Archive, ng_inode: read::Node<'_>)
    -> anyhow::Result<u32>
{
    assert!(ng_inode.is_dir()?);

    let archive_dir = ng_inode.into_owned_dir()?;

    let mut total = 0;
    for r in archive_dir {
        let node = r?;
        // If the inode represents a directory, recurse to compare the directory contents
        if node.is_dir()? {
            total += read_and_descend_ng(&archive, node)?;
        }
        total += 1;
    }
    Ok(total)
}

fn root_benchmark(c: &mut Criterion) {
    c.bench_function("Sq Read Root Dir", |b| b.iter(|| read_root_sqfs()));
    c.bench_function("Ng Read Root Dir", |b| b.iter(|| read_root_ng()));
}

fn tree_benchmark(c: &mut Criterion) {
    c.bench_function("Sq Read Tree", |b| b.iter(|| read_tree_sqfs()));
    c.bench_function("Ng Read Tree", |b| b.iter(|| read_tree_ng()));
}

criterion_group!(benches, root_benchmark, tree_benchmark);
criterion_main!(benches);