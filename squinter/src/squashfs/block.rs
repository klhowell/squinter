use std::cmp;
use std::collections::HashMap;
use std::io::{self, Cursor, Read, Seek, SeekFrom};

use super::readermux::{ReaderClient, ReaderMux};
use super::compressed::CompressedBlockReader;
use super::superblock::Compressor;

/// A seekable reader that wraps a forward-only backing-store reader. A simple Cursor provides the 
/// external reader, but the inner reader is read and appended to the Cursor whenever the requested
/// read would advance beyond the end of the Cursor's data.
/// When BorrowedBuf comes off nightly, it may be appropriate to use for this.
struct CachingReader<R: Read> {
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

impl<R:Read> Seek for CachingReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.cache.seek(pos)
    }
}


/// A store of fragment blocks. Users can request a specific fragment by block, offset, and length
/// The cache will provide a reader that is backed by the memory buffer and will read additional
/// data from the inner reader as needed to fulfill reads.
struct FragmentBlockCache<R: Read + Seek> {
    inner: ReaderMux<R>,
    block_readers: HashMap<u64, ReaderMux<CachingReader<CompressedBlockReader<ReaderClient<R>>>>>,
}

impl<R:Read+Seek> FragmentBlockCache<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner: ReaderMux::new(inner),
            block_readers: HashMap::new(),
        }
    }

    pub fn get_fragment_reader(&mut self, c: Compressor, block_addr: u64, block_size: u64, block_uncompressed_size: u64, offset: u64, len: u64)
        -> io::Result<FragmentReader<ReaderClient<CachingReader<CompressedBlockReader<ReaderClient<R>>>>>>
    {
        let block_reader = match self.block_readers.get_mut(&block_addr) {
            Some(r) => r,
            None => {
                let r = self.create_block_reader(c, block_addr, block_size, block_uncompressed_size)?;
                self.block_readers.insert(block_addr, r);
                self.block_readers.get_mut(&block_addr).unwrap()
            },
        };
        FragmentReader::new(block_reader.client(), offset, len)
    }

    fn create_block_reader(&mut self, c: Compressor, block_addr: u64, block_size: u64, uncompressed_size: u64)
        -> io::Result<ReaderMux<CachingReader<CompressedBlockReader<ReaderClient<R>>>>>
    {
        let mut client_reader = self.inner.client();
        client_reader.seek(SeekFrom::Start(block_addr))?;
        let compressed_reader = CompressedBlockReader::new(client_reader, c, block_size, uncompressed_size)?;
        let caching_reader = CachingReader::new_with_capacity(compressed_reader, block_size as usize);
        Ok(ReaderMux::new(caching_reader))
    }
}

/// A fragment reader is a single-fragment view of a portion of a fragment block
struct FragmentReader<R: Read + Seek> {
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

impl<R:Read+Seek> Seek for FragmentReader<R> {
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
        let mut block_cache = FragmentBlockCache::new(backing_reader);
        let mut buf = [0; 8];

        let mut frag_reader1 = block_cache.get_fragment_reader(Compressor::None, 32, 32, 32, 8, 16)?;
        let mut frag_reader2 = block_cache.get_fragment_reader(Compressor::None, 0, 32, 32, 0, 32)?;

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