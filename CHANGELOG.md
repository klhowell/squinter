# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.1] - 2025-03-04

### Fixed

- Build failed with ruzstd feature disabled

## [0.3.0] - 2025-02-18

### Added

- FragmentBlockCache retains uncompressed fragment blocks for reuse
- Self-contained tree-walking examples
- Shorter single-file and subtree file read benchmarks

### Fixed

- ng_read test for comparing file data to Squash-ng would not fail on short reads

### Changed

- All decompression is now combined into a single CompressedBlockReader
- Reimplemented ruzstd StreamDecoder without structure 'Read' trait bound to remove downstream 'Read' bound requirements
- When using the SquashFS::open(File) instantiation, wrap the File in a BufReader to improve performance

## [0.2.0] - 2025-02-02

### Added

- XZ Support
- ZSTD Support
- Feature flags for compressor selection

## [0.1.0] - 2025-02-01

Initial release

[0.3.1]: https://github.com/klhowell/squinter/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/klhowell/squinter/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/klhowell/squinter/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/klhowell/squinter/releases/tag/v0.1.0
