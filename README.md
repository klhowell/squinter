# squinter

A **Squ**ashFS **inter**face library written in Rust. Squinter is designed to provide live, dynamic,
read-only access to the contents of a SquashFS filesystem in the fashion that might be expected of
a mounted OS filesystem. The API focuses on performing typical filesystem operations such as listing
directory contents, reading files, etc.

## Status
Squinter has not been tested with a wide variety of inputs, but its output file tree matches that of
squashfs-ng for the few sources that have been tested. It can be used to find files and
read their properties and contents. Other than several compression algorithms, the only part of the
SquashFS specification that is known to not be supported is extended attributes.

Squinter is still experimental and should not be considered ready for production use.

## Usage
Add the following to your `Cargo.toml`:
```toml
squinter = "0.1.0"
```

```rust
use squinter::squashfs::SquashFS;
fn print_file_from_squashfs() {
    let sqfs = SquashFS::open("rootfs.squashfs")?;
    let file_reader = sqfs.open_file("/etc/group")?;
    
    let mut stdout = io::stdout().lock();
    io::copy(&mut file_reader, &mut stdout)?;
}
```

See squinter-cli for a simple unsquashfs-like code sample.

## Compressor Support
Rust has a number of quality compression libraries, but squinter has focused on handling the
SquashFS format itself. Support for more compressors will be coming soon:

| Compression Algorithm | Supported |
|-----------------------|:---------:|
| gzip                  | &check;   |
| lzma                  | &cross;   |
| lzo                   | &cross;   |
| xz                    | &cross;   |
| lz4                   | &cross;   |
| zstd                  | &cross;   |

## Performance
Squinter is designed to be a thin accessor for SquashFS content and seeks to minimize any extra
processing, read-ahead, or other pro-active optimization of what the user may want to do next. The
only non-passthrough functionality is a cache of previously decompressed metadata. As a result,
squinter should perform very well on the basis of overhead, but perhaps less well for defined
tasks like full filesystem extraction.

Limited performance benches currently consist of surfing the directory tree of a reference SquashFS
image. In these benches, squinter performs more than 10x faster than squashfs-ng. This
performance difference is too great to be a credible indicator for general usage performance, but
at least squinter doesn't appear to be a lagard.

For the below results, the reference SquashFS image was extracted from
openwrt-23.05.5-layerscape-armv8_64b-fsl_ls1012a-rdb-squashfs-firmware.bin
and then recompressed from xz to gzip with squashfs-tools. 'cargo bench' was run on an AMD 5700U
@ 1400MHz w/ Samsung 980 PRO NVMe

Time to open SquashFS image and read dir entries from root directory:
| Library     | Time     |
|-------------|----------|
| squinter    | 46us     |
| squashfs-ng | 59us     |

Time to open SquashFS image and recursively read dir entries from all directories:
| Library     | Time     |
|-------------|----------|
| squinter    | 777us    |
| squashfs-ng | 11ms     |

## Credits
Squinter was written by Kyle Howell, and is entirely based on the on-disk specification documented
here:

https://dr-emann.github.io/squashfs

All credit to Zachary Dremann and David Oberhollenzer for their excellent reverse-engineering and
documentation work.
