use std::borrow::BorrowMut;
use std::io::{self, BufReader, Read, Take, Cursor};
use std::fmt::Debug;
use std::mem;

#[cfg(feature = "flate2")]
use flate2::read::ZlibDecoder;

#[cfg(feature = "lzma-rs")]
use lzma_rs::xz_decompress;

use ruzstd::decoding::errors::FrameDecoderError;
use ruzstd::decoding::BlockDecodingStrategy;
#[cfg(feature = "ruzstd")]
use ruzstd::decoding::FrameDecoder;

use super::superblock::Compressor;

#[allow(dead_code)]
pub enum CompressedBlockReader<R>
{
    None,
    Base(R),
    Uncompressed(Take<R>),
    Buffer((R, Cursor<Vec<u8>>)),
    #[cfg(feature = "flate2")]
    Gzip(Take<ZlibDecoder<Take<R>>>),
    #[cfg(feature = "ruzstd")]
    Zstd(Take<ZstdDecoder<Take<R>, FrameDecoder>>),
}

impl<R: Read> Debug for CompressedBlockReader<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "None"),
            Self::Base(_) => write!(f, "Base"),
            Self::Uncompressed(_) => write!(f, "Uncompressed"),
            Self::Buffer(_) => write!(f, "Buffer"),
            #[cfg(feature = "flate2")]
            Self::Gzip(_) => write!(f, "Gzip"),
            #[cfg(feature = "ruzstd")]
            Self::Zstd(_) => write!(f, "Zstd"),
        }
    }
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
                let dec = ZstdDecoder::new(block_reader)
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

/// This struct is a near-copy of the StreamingDecoder from ruzstd. The only difference is the
/// removal of the 'Read' constraint on generic R of the struct, which causes this constraint
/// to propagate all over squinter's structs. Removing the constraint allows the Read constraint
/// to be limited to impls.
#[cfg(feature = "ruzstd")]
pub struct ZstdDecoder<READ, DEC: BorrowMut<FrameDecoder>> {
    pub decoder: DEC,
    source: READ,
}

#[cfg(feature = "ruzstd")]
impl<READ: Read, DEC: BorrowMut<FrameDecoder>> ZstdDecoder<READ, DEC> {
    pub fn new_with_decoder(
        mut source: READ,
        mut decoder: DEC,
    ) -> Result<ZstdDecoder<READ, DEC>, FrameDecoderError> {
        decoder.borrow_mut().init(&mut source)?;
        Ok(ZstdDecoder { decoder, source })
    }
}

#[cfg(feature = "ruzstd")]
impl<READ: Read> ZstdDecoder<READ, FrameDecoder> {
    pub fn new(
        mut source: READ,
    ) -> Result<ZstdDecoder<READ, FrameDecoder>, FrameDecoderError> {
        let mut decoder = FrameDecoder::new();
        decoder.init(&mut source)?;
        Ok(ZstdDecoder { decoder, source })
    }
}

#[cfg(feature = "ruzstd")]
impl<READ: Read, DEC: BorrowMut<FrameDecoder>> ZstdDecoder<READ, DEC> {
    /// Gets a reference to the underlying reader.
    pub fn get_ref(&self) -> &READ {
        &self.source
    }

    /// Gets a mutable reference to the underlying reader.
    ///
    /// It is inadvisable to directly read from the underlying reader.
    pub fn get_mut(&mut self) -> &mut READ {
        &mut self.source
    }

    /// Destructures this object into the inner reader.
    pub fn into_inner(self) -> READ
    where
        READ: Sized,
    {
        self.source
    }

    /// Destructures this object into both the inner reader and [FrameDecoder].
    pub fn into_parts(self) -> (READ, DEC)
    where
        READ: Sized,
    {
        (self.source, self.decoder)
    }

    /// Destructures this object into the inner [FrameDecoder].
    pub fn into_frame_decoder(self) -> DEC {
        self.decoder
    }
}

#[cfg(feature = "ruzstd")]
impl<READ: Read, DEC: BorrowMut<FrameDecoder>> Read for ZstdDecoder<READ, DEC> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let decoder = self.decoder.borrow_mut();
        if decoder.is_finished() && decoder.can_collect() == 0 {
            //No more bytes can ever be decoded
            return Ok(0);
        }

        // need to loop. The UpToBytes strategy doesn't take any effort to actually reach that limit.
        // The first few calls can result in just filling the decode buffer but these bytes can not be collected.
        // So we need to call this until we can actually collect enough bytes

        // TODO add BlockDecodingStrategy::UntilCollectable(usize) that pushes this logic into the decode_blocks function
        while decoder.can_collect() < buf.len() && !decoder.is_finished() {
            //More bytes can be decoded
            let additional_bytes_needed = buf.len() - decoder.can_collect();
            match decoder.decode_blocks(
                &mut self.source,
                BlockDecodingStrategy::UptoBytes(additional_bytes_needed),
            ) {
                Ok(_) => { /*Nothing to do*/ }
                Err(e) => {
                    let err = io::Error::new(io::ErrorKind::Other, e);
                    return Err(err);
                }
            }
        }

        decoder.read(buf)
    }
}
