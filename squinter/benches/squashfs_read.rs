use std::time::Duration;
use std::io::{Read, Seek};
use std::path::Path;

use anyhow;
use criterion::{criterion_group, criterion_main, Criterion, BatchSize};
use test_assets_ureq::{TestAssetDef, dl_test_files_backoff};

use squashfs_ng::read::{self, Archive};
use squinter::squashfs::{self, SquashFS};
const TEST_DATA_DIR: &str = "../test_data";
const TEST_IMG_SRC: &str = "https://downloads.openwrt.org/releases/23.05.5/targets/layerscape/armv8_64b";
const TEST_IMG_NAME: &str = "openwrt-23.05.5-layerscape-armv8_64b-fsl_ls1012a-rdb-squashfs-firmware.bin";
const TEST_IMG_HASH: &str = "405331d0e203da3877f47934205e29a8835525e3deb1bd9c966e5102a05bc9a7";
const TEST_SQUASH_NAME: &str = "test.squashfs";
const TEST_SQUASH_OFFSET: u64 = 0x2000000;
const TEST_SQUASH_LEN: Option<u64> = None;

const COMPRESSION_METHODS: [&str;3] = ["gzip", "xz", "zstd"];

fn read_root_sqfs(test_file: &str) -> anyhow::Result<usize> {

    let mut sqfs = squashfs::SquashFS::open(test_file)?;
    let root_count = sqfs.read_dir("/")?
        .count();

    Ok(root_count)
}

fn read_root_ng(test_file: &str) -> anyhow::Result<usize> {
    let archive = read::Archive::open(test_file)?;
    let root_count = archive.get_exists("/")?.into_owned_dir()?
        .count();

    Ok(root_count)
}

fn read_tree_sqfs(test_file: &str, content: bool) -> anyhow::Result<u32> {
    let mut sqfs = squashfs::SquashFS::open(test_file)?;
    let sqfs_rootnode = sqfs.root_inode()?;
    let total = read_and_descend_sqfs(&mut sqfs, &sqfs_rootnode, content)?;
    Ok(total)
}

fn read_tree_ng(test_file: &str, content: bool) -> anyhow::Result<u32> {
    let archive = read::Archive::open(test_file)?;
    let archive_rootnode = archive.get_exists("/")?;
    let total = read_and_descend_ng(&archive, archive_rootnode, content)?;
    Ok(total)
}

fn read_and_descend_sqfs<R: Read + Seek>(sqfs: &mut squashfs::SquashFS<R>, sq_inode: &squashfs::metadata::Inode, content: bool)
    -> anyhow::Result<u32>
{
    assert!(sq_inode.is_dir());

    let sqfs_dir = sqfs.read_dir_inode(sq_inode)?;

    let mut total = 0;
    for de in sqfs_dir {
        let sq_inode = sqfs.inode_from_entryref(de.inode_ref())?;
        if content && sq_inode.is_file() {
            let mut sq_reader = sqfs.open_file_inode(&sq_inode)?;
            std::io::copy(&mut sq_reader, &mut std::io::sink())?;
        }
        // If the inode represents a directory, recurse to the directory contents
        if sq_inode.is_dir() {
            total += read_and_descend_sqfs(sqfs, &sq_inode, content)?;
        }
        total += 1;
    }
    Ok(total)
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

fn read_single_sqfs<R: Read + Seek>(sqfs: &mut squashfs::SquashFS<R>, path: &Path) -> anyhow::Result<u64> {
    let mut r = sqfs.open_file(path)?;
    let total = std::io::copy(&mut r, &mut std::io::sink())?;
    Ok(total)
}

fn read_single_ng(archive: &read::Archive, path: &Path) -> anyhow::Result<u64> {
    let node = archive.get_exists(path)?;
    let mut r = node.as_file()?;
    let total = std::io::copy(&mut r, &mut std::io::sink())?;
    Ok(total)
}

fn root_benchmark(c: &mut Criterion) {
    prepare_test_files().unwrap();
    for comp in COMPRESSION_METHODS {
        let test_file = format!("{TEST_DATA_DIR}/test.{comp}.squashfs");
        let group_name = format!("{comp} - Read Root Dir");
        let mut group = c.benchmark_group(&group_name);
        group.sample_size(100);
        group.bench_function(&format!("Squinter"), |b| b.iter(|| read_root_sqfs(&test_file)));
        group.bench_function(&format!("Squashfs-ng"), |b| b.iter(|| read_root_ng(&test_file)));
        group.finish();
    }
}

fn tree_benchmark(c: &mut Criterion) {
    prepare_test_files().unwrap();
    for comp in COMPRESSION_METHODS {
        let test_file = format!("{TEST_DATA_DIR}/test.{comp}.squashfs");
        let group_name = format!("{comp} - Read Tree");
        let mut group = c.benchmark_group(&group_name);
        group.sample_size(100);
        group.bench_function(&format!("Squinter"), |b|
            b.iter(|| read_tree_sqfs(&test_file, false)));
        group.bench_function(&format!("Squashfs-ng"), |b|
            b.iter(|| read_tree_ng(&test_file, false)));
        group.finish();
    }
}

fn data_benchmark(c: &mut Criterion) {
    prepare_test_files().unwrap();
    for comp in COMPRESSION_METHODS {
        let test_file = format!("{TEST_DATA_DIR}/test.{comp}.squashfs");
        let group_name = format!("{comp} - Read Files");
        let mut group = c.benchmark_group(&group_name);
        group.sample_size(10);
        group.bench_function(&format!("Squinter"), |b|
            b.iter(|| read_tree_sqfs(&test_file, true)));
        group.bench_function(&format!("Squashfs-ng"), |b|
            b.iter(|| read_tree_ng(&test_file, true)));
        group.finish();
    }
}

fn single_file_benchmark(c: &mut Criterion) {
    prepare_test_files().unwrap();
    //let p = Path::new("/lib/libc.so");
    let p = Path::new("/www/luci-static/resources/fs.js");
    for comp in COMPRESSION_METHODS {
        let test_file = format!("{TEST_DATA_DIR}/test.{comp}.squashfs");
        let group_name = format!("{comp} - Read Single");
        let mut group = c.benchmark_group(&group_name);
        group.sample_size(10);
        group.bench_function(&format!("Squinter"), |b|
            b.iter_batched(||
                SquashFS::open(&test_file).unwrap(),
                |mut sqfs| read_single_sqfs(&mut sqfs, p).unwrap(),
                BatchSize::PerIteration));

        group.bench_function(&format!("Squashfs-ng"), |b|
            b.iter_batched(||
                Archive::open(&test_file).unwrap(),
                |mut ng| read_single_ng(&mut ng, p).unwrap(),
                BatchSize::PerIteration));
        group.finish();
    }
}

fn partial_tree_benchmark(c: &mut Criterion) {
    prepare_test_files().unwrap();
    //let p = Path::new("/lib");
    let p = Path::new("/www/luci-static/resources");
    for comp in COMPRESSION_METHODS {
        let test_file = format!("{TEST_DATA_DIR}/test.{comp}.squashfs");
        let group_name = format!("{comp} - Partial Tree");
        let mut group = c.benchmark_group(&group_name);
        group.sample_size(10);
        group.bench_function(&format!("Squinter"), |b|
            b.iter_batched(||
                SquashFS::open(&test_file).unwrap(),
                |mut sqfs| {
                    let i = sqfs.inode_from_path(p).unwrap();
                    read_and_descend_sqfs(&mut sqfs, &i, true).unwrap();
                },
                BatchSize::PerIteration));

        group.bench_function(&format!("Squashfs-ng"), |b|
            b.iter_batched(||
                Archive::open(&test_file).unwrap(),
                |ng| {
                    let n = ng.get_exists(p).unwrap();
                    read_and_descend_ng(&ng, n, true).unwrap();
                },
                BatchSize::PerIteration));
        group.finish();
    }
}

criterion_group!(benches, root_benchmark, tree_benchmark, data_benchmark, single_file_benchmark, partial_tree_benchmark);
criterion_main!(benches);

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
