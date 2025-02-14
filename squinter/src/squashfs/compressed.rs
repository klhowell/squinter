use std::io::{self, BufReader, Read, Take, Cursor};
use std::mem;

#[cfg(feature = "flate2")]
use flate2::read::ZlibDecoder;

#[cfg(feature = "lzma-rs")]
use lzma_rs::xz_decompress;

#[cfg(feature = "ruzstd")]
use ruzstd::decoding::{FrameDecoder, StreamingDecoder};

use super::superblock::Compressor;

#[allow(dead_code)]
pub enum CompressedBlockReader<R>
where
    R: Read,
{
    None,
    Base(R),
    Uncompressed(Take<R>),
    Buffer((R, Cursor<Vec<u8>>)),
    #[cfg(feature = "flate2")]
    Gzip(Take<ZlibDecoder<Take<R>>>),
    #[cfg(feature = "ruzstd")]
    Zstd(Take<StreamingDecoder<Take<R>, FrameDecoder>>),
}

#[allow(dead_code)]
impl<R: Read> CompressedBlockReader<R> {
    pub fn new(r: R, comp: Compressor, compressed_size: u64, uncompressed_size: u64) -> io::Result<Self> {
        let block_reader = r.take(compressed_size);
        Ok(match comp {
            Compressor::None => {
                CompressedBlockReader::Uncompressed(block_reader)
            },
            #[cfg(feature = "flate2")]
            Compressor::Gzip => {
                let dec = ZlibDecoder::new(block_reader).take(uncompressed_size);
                CompressedBlockReader::Gzip(dec)
            },
            #[cfg(feature = "lzma-rs")]
            Compressor::Xz => {
                // The xz decompressor doesn't support incremental reading, so uncompress the whole
                // block into a buffer and use a Cursor as the reader.
                // TODO: For fragments, this is uncompressing the entire block just for a small portion and then
                // throwing the rest of the uncompressed data away afterwards. Consider an option to retain uncompressed
                // data like the CachedMetadataReader does.
                let buf = Vec::with_capacity(uncompressed_size as usize);
                let mut buf_reader = BufReader::new(block_reader);
                let mut buf_writer = Cursor::new(buf);
                xz_decompress(&mut buf_reader, &mut buf_writer)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                buf_writer.set_position(0);
                let r = buf_reader.into_inner().into_inner();
                CompressedBlockReader::Buffer((r, buf_writer))
            },
            #[cfg(feature = "ruzstd")]
            Compressor::Zstd => {
                let dec = StreamingDecoder::new(block_reader)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
                    .take(uncompressed_size);
                CompressedBlockReader::Zstd(dec)
            },
            _ => { return Err(io::Error::from(io::ErrorKind::Unsupported)) },
        })
    }

    pub fn into_inner(self) -> R {
        match self {
            CompressedBlockReader::Base(r) => r,
            CompressedBlockReader::Uncompressed(r) => r.into_inner(),
            CompressedBlockReader::Buffer(r) => r.0,
            #[cfg(feature="flate2")]
            CompressedBlockReader::Gzip(r) => r.into_inner().into_inner().into_inner(),
            #[cfg(feature="ruzstd")]
            CompressedBlockReader::Zstd(r) => r.into_inner().into_inner().into_inner(),
            CompressedBlockReader::None => panic!("into_inner: no inner value"),
        }
    }

    pub fn into_base(self) -> Self {
        CompressedBlockReader::Base(self.into_inner())
    }

    pub fn take(&mut self) -> CompressedBlockReader<R> {
        mem::replace(self, CompressedBlockReader::None)
    }
}

impl<R: Read> Read for CompressedBlockReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            CompressedBlockReader::Base(r) => r.read(buf),
            CompressedBlockReader::Uncompressed(r) => r.read(buf),
            CompressedBlockReader::Buffer(r) => r.1.read(buf),
            #[cfg(feature="flate2")]
            CompressedBlockReader::Gzip(r) => r.read(buf),
            #[cfg(feature="ruzstd")]
            CompressedBlockReader::Zstd(r) => r.read(buf),
            _ => panic!("CompressedBlockReader::read: no inner value"),
        }
    }

}
