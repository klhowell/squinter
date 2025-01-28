# squinter

A **Squ**ashFS **inter**face library written in Rust. Squinter is designed to provide live, dynamic,
read-only access to the contents of a SquashFS filesystem in the fashion that might be expected of
a mounted OS filesystem. The API focuses on performing typical filesystem operations such as listing
directory contents, reading files, etc.

## Status
Squinter has not been tested with a wide variety of inputs, but its file tree matches that of
squashfs-tools-ng for the few sources that have been tested. It can be used to find files and
read their properties and contents. The only part of the SquashFS specification that is known
to not be supported is extended attributes.

Squinter is still experimental and should not be considered ready for production use.

## Usage
Add the following to your `Cargo.toml`:
```toml
squinter = "0.1.0"
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
target. In these benches, squinter performs approximately 10x faster than squashfs-ng. This
performance difference is too great to be a credible indicator for general usage performance, but
at least squinter doesn't appear to be a lagard.

For the below results, the reference SquashFS image was extracted from
openwrt-23.05.5-layerscape-armv8_64b-fsl_ls1012a-rdb-squashfs-firmware.bin
and then recompressed from xz to gzip with squashfs-tools.

Time to open SquashFS image and read dir entries from root directory:
| Library     | Time     |
|-------------|----------|
| squinter    | 46.289us |
| squashfs-ng | 59.871us |

Time to open SquashFS image and recursively read dir entries from all directories:
| Library     | Time     |
|-------------|----------|
| squinter    | 777.55us |
| squashfs-ng | 11.246ms |

