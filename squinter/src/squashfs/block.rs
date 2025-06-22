use std::cell::RefCell;
use std::cmp;
use std::collections::HashMap;
use std::io::{self, Cursor, Read, Seek, SeekFrom};

use byteorder::{LittleEndian, ReadBytesExt};

use super::metadata::EntryReference;
use super::readermux::{ReaderClient, ReaderMux};
use super::compressed::CompressedBlockReader;
use super::superblock::Compressor;

/// A seekable reader that wraps a forward-only backing-store reader. A simple Cursor provides the 
/// external reader, but the inner reader is read and appended to the Cursor whenever the requested
/// read would advance beyond the end of the Cursor's data.
/// When BorrowedBuf comes off nightly, it may be appropriate to use for this.
#[derive(Debug)]
pub struct CachingReader<R> {
    inner: R,
    cache: Cursor<Vec<u8>>,
}

impl<R:Read> CachingReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner: inner,
            cache: Cursor::new(Vec::new()),
        }
    }

    pub fn new_with_capacity(inner: R, cap: usize) -> Self {
        Self {
            inner: inner,
            cache: Cursor::new(Vec::with_capacity(cap)),
        }
    }
}

impl<R:Read> Read for CachingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let initial_pos = self.cache.position();
        let end_pos = initial_pos + (buf.len() as u64);

        // If we need to read more inner data, first do that and add it to the cache
        if end_pos > self.cache.get_ref().len() as u64 {
            let target_read_len = end_pos - (self.cache.get_ref().len() as u64);
            self.cache.seek(SeekFrom::End(0))?;
            io::copy(&mut self.inner.by_ref().take(target_read_len), &mut self.cache)?;
            self.cache.set_position(initial_pos);
        }

        self.cache.read(buf)
    }
}

impl<R> Seek for CachingReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.cache.seek(pos)
    }
}


/// A store of fragment blocks. Users can request a specific fragment by block, offset, and length
/// The cache will provide a reader that is backed by the memory buffer and will read additional
/// data from the inner reader as needed to fulfill reads.
#[derive(Debug)]
pub struct FragmentBlockCache<R: Read+Seek> {
    inner: ReaderMux<R>,
    compressor: Compressor,
    block_readers: HashMap<u64, ReaderMux<CachingReader<CompressedBlockReader<ReaderClient<R>>>>>,
}

impl<R:Read+Seek> FragmentBlockCache<R> {
    pub fn new(inner: R, compressor: Compressor) -> Self {
        Self {
            inner: ReaderMux::new(inner),
            compressor,
            block_readers: HashMap::new(),
        }
    }

    pub fn get_fragment_reader(&mut self, block_addr: u64, block_size: u64, block_uncompressed_size: u64, offset: u64, len: u64)
        -> io::Result<FragmentReader<ReaderClient<CachingReader<CompressedBlockReader<ReaderClient<R>>>>>>
    {
        let block_reader = match self.block_readers.get_mut(&block_addr) {
            Some(r) => r,
            None => {
                let r = self.create_block_reader(block_addr, block_size, block_uncompressed_size)?;
                self.block_readers.insert(block_addr, r);
                self.block_readers.get_mut(&block_addr).unwrap()
            },
        };
        FragmentReader::new(block_reader.client(), offset, len)
    }

    fn create_block_reader(&mut self, block_addr: u64, block_size: u64, uncompressed_size: u64)
        -> io::Result<ReaderMux<CachingReader<CompressedBlockReader<ReaderClient<R>>>>>
    {
        let mut client_reader = self.inner.client();
        client_reader.seek(SeekFrom::Start(block_addr))?;
        let compressed_reader = CompressedBlockReader::new(client_reader, self.compressor, block_size, uncompressed_size)?;
        let caching_reader = CachingReader::new_with_capacity(compressed_reader, uncompressed_size as usize);
        Ok(ReaderMux::new(caching_reader))
    }
}

/// A fragment reader is a single-fragment view of a portion of a fragment block
#[derive(Debug)]
pub struct FragmentReader<R> {
    inner: R, // the backing reader
    offset: u64, // offset of the beginning of this reader within the inner reader
    len: u64, // length of this reader
    pos: u64, // 0-based position within this reader
}

impl<R:Read+Seek> FragmentReader<R> {
    pub fn new(mut inner: R, offset: u64, len: u64) -> io::Result<Self> {
        inner.seek(SeekFrom::Start(offset))?;
        Ok(Self {
            inner,
            offset,
            len,
            pos: 0,
        })
    }
}

impl<R:Read+Seek> Read for FragmentReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.len {
            return Ok(0);
        }
        let max = cmp::min(buf.len() as u64, self.len-self.pos) as usize;
        let n = self.inner.read(&mut buf[..max])?;
        assert!(self.pos + n as u64 <= self.len);
        self.pos += n as u64;
        Ok(n)
    }
}

impl<R:Seek> Seek for FragmentReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match pos {
            SeekFrom::Start(p) => {
                let n = self.inner.seek(SeekFrom::Start(self.offset+p))?;
                assert!(n >= self.offset);
                self.pos = n - self.offset;
                Ok(self.pos)
            },
            SeekFrom::End(p) => {
                assert!(self.len as i64 + p >= 0);
                let inner_pos = (self.offset+self.len) as i64 + p;
                let n = self.inner.seek(SeekFrom::Start(inner_pos as u64))?;
                assert!(n >= self.offset);
                self.pos = n - self.offset;
                Ok(self.pos)
            },
            SeekFrom::Current(p) => {
                assert!(self.pos as i64 + p >= 0);
                let n = self.inner.seek(SeekFrom::Current(p))?;
                assert!(n >= self.offset);
                self.pos = n - self.offset;
                Ok(self.pos)
            },
        }
    }
}

/// A store of metadata blocks. Users can request specific data by block address.
/// The cache will provide a reader that is backed by the memory buffer and will read additional
/// data from the inner reader as needed to fulfill reads. Provided readers will automatically
/// advance to the next metadata block as needed.
#[derive(Debug)]
pub struct MetadataBlockCache<R: Read+Seek> {
    inner: RefCell<ReaderMux<R>>,
    compressor: Compressor,
    block_readers: RefCell<HashMap<u64, MetadataBlockReaderMux<ReaderClient<R>>>>,
}

impl<R:Read+Seek> MetadataBlockCache<R> {
    pub fn new(inner: R, compressor: Compressor) -> Self {
        Self {
            inner: RefCell::new(ReaderMux::new(inner)),
            compressor,
            block_readers: RefCell::new(HashMap::new()),
        }
    }

    // Create and return a new MetadataBlockReader over the metadata block at the specified
    // address, creating a new caching ReaderMux to back it if necessary. This Reader only spans a
    // single block and will not automatically roll into the next block.
    fn get_block_reader(&self, block_addr: u64)
        -> io::Result<MetadataBlockReader<ReaderClient<R>>>
    {
        let mut block_readers = self.block_readers.borrow_mut();
        let block_reader_mux = match block_readers.get_mut(&block_addr) {
            Some(r) => r,
            None => {
                //let r = self.create_block_reader(block_addr)?;
                let r = MetadataBlockReaderMux::new(
                    self.inner.borrow_mut().client(),
                    block_addr,
                    self.compressor,
                )?;
                block_readers.insert(block_addr, r);
                block_readers.get_mut(&block_addr).unwrap()
            },
        };
        Ok(block_reader_mux.client())
    }
}

#[derive(Debug)]
struct MetadataBlockReaderMux<R:Read+Seek> {
    inner: ReaderMux<CachingReader<CompressedBlockReader<R>>>,
    block_addr: u64,
    block_size: u16,
}

impl<R:Read+Seek> MetadataBlockReaderMux<R> {
    fn new(mut inner: R, block_addr: u64, compressor: Compressor) -> io::Result<Self> {
        // SquashFS Metadata block size is fixed by the specification
        const METADATA_UNCOMPRESSED_BLOCK_SIZE: u16 = 8192;

        inner.seek(SeekFrom::Start(block_addr))?;

        // The size of metadata blocks is stored in a 16-bit header
        let header: u16 = inner.read_u16::<LittleEndian>()?;
        let block_size: u16 = header & 0x7FFF;
        let compressed: bool = header & 0x8000 == 0;
        
        if block_size > METADATA_UNCOMPRESSED_BLOCK_SIZE {
            eprintln!("Metadata block size too big -- {}", block_size);
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        
        let compressor = if compressed {
            compressor
        } else {
            Compressor::None
        };

        let compressed_reader = CompressedBlockReader::new(inner, compressor, block_size.into(), METADATA_UNCOMPRESSED_BLOCK_SIZE.into())?;
        let caching_reader = CachingReader::new_with_capacity(compressed_reader, METADATA_UNCOMPRESSED_BLOCK_SIZE.into());
        let reader_mux = ReaderMux::new(caching_reader);
        Ok(
            Self {
                inner: reader_mux,
                block_addr,
                block_size,
            }
        )
    }
    
    fn client(&mut self) -> MetadataBlockReader<R> {
        let reader_client = self.inner.client();
        MetadataBlockReader::new(reader_client, self.block_addr, self.block_size)
    }
}

#[derive(Debug)]
pub struct MetadataBlockReader<R:Read+Seek> {
    inner: ReaderClient<CachingReader<CompressedBlockReader<R>>>,
    block_addr: u64,
    block_size: u16,
}

impl<R:Read+Seek> MetadataBlockReader<R> {
    pub fn new(inner: ReaderClient<CachingReader<CompressedBlockReader<R>>>, block_addr: u64, block_size: u16) -> Self {
        Self {
            inner,
            block_addr,
            block_size,
        }
    }
    
    pub fn block_addr(&self) -> u64 {
        self.block_addr
    }
    
    pub fn block_size(&self) -> u16 {
        self.block_size
    }
    
    // Don't forget about the 2-byte header
    pub fn next_block_addr(&self) -> u64 {
        self.block_addr + 2 + self.block_size as u64
    }
}

impl<R:Read+Seek> Read for MetadataBlockReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R:Read+Seek> Seek for MetadataBlockReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

pub struct MetadataReader<'a, R:Read+Seek> {
    cache: &'a MetadataBlockCache<R>,
    inner: MetadataBlockReader<ReaderClient<R>>,
    section_start: u64,
    section_end: Option<u64>,
}

impl<'a, R:Read+Seek> MetadataReader<'a, R> {
    pub fn new(cache: &'a MetadataBlockCache<R>, section_start: u64, section_end: Option<u64>, entry_addr: EntryReference) -> io::Result<Self> {
        let mut inner = cache.get_block_reader(section_start + entry_addr.location())?;
        inner.seek(SeekFrom::Current(entry_addr.offset().into()))?;
        Ok(Self {
            cache,
            inner,
            section_start,
            section_end,
        })
    }
    
    pub fn seek_ref(&mut self, addr: EntryReference) -> io::Result<()> {
        if self.section_start + addr.location() != self.inner.block_addr() {
            self.inner = self.cache.get_block_reader(self.section_start + addr.location())?;
        }
        self.inner.seek(SeekFrom::Start(addr.offset().into()))?;
        Ok(())
    }
}

impl<'a, R:Read+Seek> Read for MetadataReader<'a, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let size = self.inner.read(buf)?;
        if size == 0 && buf.len() != 0 {
            // This must be the end of the block. Potentially start a new one.
            let done = self.section_end.is_some_and(|x| x <= self.inner.next_block_addr());
            if done {
                Ok(0)
            } else {
                self.inner = self.cache.get_block_reader(self.inner.next_block_addr())?;
                self.inner.read(buf)
            }
        } else {
            Ok(size)
        }
    }
}

impl<'a, R:Read+Seek> Seek for MetadataReader<'a, R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_caching_reader() -> io::Result<()> {
        let data: Vec<u8> = (0..=255).collect();
        let backing_reader = Cursor::new((0..=255).collect::<Vec<u8>>());
        let mut caching_reader = CachingReader::new(backing_reader);
        let mut buf = [0; 8];

        assert_eq!(caching_reader.read(&mut buf)?, buf.len());
        println!("Read {:?}", &buf);
        assert!(buf.iter().eq(data[0..8].iter()));

        assert_eq!(caching_reader.read(&mut buf)?, buf.len());
        println!("Read {:?}", &buf);
        assert!(buf.iter().eq(data[8..16].iter()));

        caching_reader.seek(SeekFrom::Current(8))?;
        assert_eq!(caching_reader.read(&mut buf)?, buf.len());
        println!("Read {:?}", &buf);
        assert!(buf.iter().eq(data[24..32].iter()));

        caching_reader.seek(SeekFrom::Current(-16))?;
        assert_eq!(caching_reader.read(&mut buf)?, buf.len());
        println!("Read {:?}", &buf);
        assert!(buf.iter().eq(data[16..24].iter()));

        caching_reader.seek(SeekFrom::Start(252))?;
        assert_eq!(caching_reader.read(&mut buf)?, 4);
        println!("Read {:?}", &buf[0..4]);
        assert!(buf.iter().take(4).eq(data[252..=255].iter()));
        Ok(())
    }

    #[test]
    fn test_block_cache() -> io::Result<()> {
        let data: Vec<u8> = (0..=255).collect();
        let backing_reader = Cursor::new((0..=255).collect::<Vec<u8>>());
        let mut block_cache = FragmentBlockCache::new(backing_reader, Compressor::None);
        let mut buf = [0; 8];

        let mut frag_reader1 = block_cache.get_fragment_reader(32, 32, 32, 8, 16)?;
        let mut frag_reader2 = block_cache.get_fragment_reader(0, 32, 32, 0, 32)?;

        assert_eq!(frag_reader1.read(&mut buf)?, buf.len());
        println!("Read {:?}", &buf);
        assert!(buf.iter().eq(data[40..48].iter()));

        assert_eq!(frag_reader2.read(&mut buf)?, buf.len());
        println!("Read {:?}", &buf);
        assert!(buf.iter().eq(data[0..8].iter()));

        assert_eq!(frag_reader1.read(&mut buf)?, buf.len());
        println!("Read {:?}", &buf);
        assert!(buf.iter().eq(data[48..56].iter()));

        assert_eq!(frag_reader2.read(&mut buf)?, buf.len());
        println!("Read {:?}", &buf);
        assert!(buf.iter().eq(data[8..16].iter()));

        Ok(())
    }

}