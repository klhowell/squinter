# Squinter &emsp; [![Latest Version]][crates.io] [![Documentation]][docs.rs]

[Latest Version]: https://img.shields.io/crates/v/squinter.svg
[crates.io]: https://crates.io/crates/squinter
[Documentation]: https://img.shields.io/badge/docs.rs-squinter-66c2a5
[docs.rs]: https://docs.rs/squinter

A **Squ**ashFS **inter**face library written in Rust. Squinter is designed to provide live, dynamic,
read-only access to the contents of a SquashFS filesystem in the fashion that might be expected of
a mounted OS filesystem. The API focuses on performing typical filesystem operations such as listing
directory contents, reading files, etc.

## Status
Squinter has not been tested with a wide variety of inputs, but its output file tree matches that of
squashfs-ng for the few sources that have been tested. It can be used to find files and
read their properties and contents. Other than several compression algorithms, the only part of the
SquashFS specification that is known to not be supported is extended attributes.

Squinter is still experimental and should not be considered ready for production use. Consumers
should expect APIs to change frequently. Only happy paths have been tested, and broken or
maliciously constructed SquashFS filesystems *will* result in bad data and/or crashes.

## Usage
Add the following to your `Cargo.toml`:
```toml
squinter = "0.1.0"
```

```rust
use std::io;
use squinter::squashfs::SquashFS;
fn print_file_from_squashfs() -> io::Result<()>{
    // Open the SquashFS
    let mut sqfs = SquashFS::open("rootfs.squashfs")?;

    // List the contents of a directory
    for d in sqfs.read_dir("/etc")? {
        println!("{}", d.file_name())
    }

    // Open a file to read its contents
    let mut file_reader = sqfs.open_file("/etc/group")?;
    
    // Copy the file contents to stdout
    let mut stdout = io::stdout().lock();
    io::copy(&mut file_reader, &mut stdout)?;
    Ok(())
}
```

See squinter-cli for a simple unsquashfs-like code sample.

## Compressor Support
Squinter intends to support any SquashFS compression algorithms that have pure Rust
implementations. Currently, the three most popular are supported:

| Compression Algorithm | Supported |
|-----------------------|:---------:|
| gzip                  | &check;   |
| lzma                  | &cross;   |
| lzo                   | &cross;   |
| xz                    | &check;   |
| lz4                   | &cross;   |
| zstd                  | &check;   |

## Performance
Squinter is designed to be a thin accessor for SquashFS content and seeks to minimize any extra
processing, read-ahead, or other pro-active optimization of what the user may want to do next. The
only non-passthrough functionality is a cache of previously decompressed metadata. As a result,
squinter should perform well on the basis of overhead, but perhaps less well for defined
tasks like full filesystem extraction. While the code has been written not to be wasteful, little
attention has been paid to maximizing performance.

Limited performance benches currently consist of surfing the directory tree of a reference SquashFS
image. When purely reading dir entries, squinter comes in more than 10x faster that squashfs-ng.
However, when file contents are also read, squinter is about 3x slower than squashfs-ng.

Compression algorithm implementations also clearly contribute to the overall performance of
Squinter. For example, experiments with turning on the 'zlib-ng' feature in flate2 yielded up to
40% data read-speed improvements. However, squashfs-ng is an excellent library for users who desire
to link a C library. Squinter will instead continue to use pure Rust compressor implementations (at
least by default) for the time being.

For the below results, the reference SquashFS image was extracted from
openwrt-23.05.5-layerscape-armv8_64b-fsl_ls1012a-rdb-squashfs-firmware.bin
and then recompressed from xz to gzip with squashfs-tools. 'cargo bench' was run on an AMD 5700U
@ 1400MHz w/ Samsung 980 PRO NVMe

| Benchmark                                 | Squashfs-ng | Squinter | Difference |
|-------------------------------------------|-------------|----------|------------|
| **gzip**: Open & read root directory      | 60us        | 46us     | -23%       |
| **xz**: Open & read root directory        | 280us       | 260us    | -7%        |
| **zstd**: Open & read root directory      | 46us        | 76us     | +65%       |
| **gzip**: Open & read full directory tree | 11ms        | 790us    | -93%       |
| **xz**: Open & read full directory tree   | 76ms        | 2.3ms    | -97%       |
| **zstd**: Open & read full directory tree | 6.6ms       | 970us    | -85%       |
| **gzip**: Open & read all file contents   | 77ms        | 210ms    | +173%      |
| **xz**: Open & read all file contents     | 360ms       | 2.40s    | +567%      |
| **zstd**: Open & read all file contents   | 29ms        | 560ms    | +1831%     |

## Credits
Squinter was written by Kyle Howell, and is entirely based on the on-disk specification documented
here:

https://dr-emann.github.io/squashfs

All credit to Zachary Dremann and David Oberhollenzer for their excellent reverse-engineering and
documentation work.
