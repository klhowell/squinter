use std::io;
use std::io::{Read, Seek, SeekFrom};

use super::block::{CachingReader, FragmentBlockCache, FragmentReader};
use super::metadata::{self, FragmentEntry, Inode, InodeExtendedInfo};
use super::readermux::{ReaderClient, ReaderMux};
use super::superblock::{Superblock, Compressor};
use super::compressed::CompressedBlockReader;

#[allow(dead_code)]
struct FileBlockInfo<R> {
    disk_offset: u64,
    disk_len: u32,
    data_offset: u32,       // How far into this block the file data starts (only applies to tail-ends)
    data_len: u32,          // Amount of uncompressed file data in this block
    is_compressed: bool,
    reader: BlockReader<R>,
}

// TODO: TailEnd support
#[allow(dead_code)]
struct TailEnd {
    fragment: FragmentEntry,
    offset: u32,
}

enum BlockReader<R> {
    Block(CachingReader<CompressedBlockReader<ReaderClient<R>>>),
    Fragment(FragmentReader<ReaderClient<CachingReader<CompressedBlockReader<ReaderClient<R>>>>>),
}

impl<R: Read + Seek> Read for BlockReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            BlockReader::Block(r) => r.read(buf),
            BlockReader::Fragment(r) => r.read(buf),
        }
    }
}

impl<R: Seek> Seek for BlockReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match self {
            BlockReader::Block(r) => r.seek(pos),
            BlockReader::Fragment(r) => r.seek(pos),
        }
    }
}

/// Reader for uncompressed file data from a series of compressed blocks and possibly a tail-end.
///
/// This reader presents a single contiguous stream to the caller. 
pub struct FileDataReader<R: Read + Seek> {
    inner: ReaderMux<R>,
    pos: u64,
    block_size: u32,
    file_size: u64,
    blocks: Vec<FileBlockInfo<R>>,
}

impl<R: Read + Seek> FileDataReader<R> {
    pub fn from_inode(inner: R, sb: &Superblock, frag_cache: &mut FragmentBlockCache<R>, inode: &Inode) -> io::Result<Option<Self>>
    {
        let pos = 0;
        let comp = sb.compressor;
        let block_size = sb.block_size;
        let mut inner = ReaderMux::new(inner);
        let mut blocks = Vec::new();
        match &inode.extended_info {
            InodeExtendedInfo::BasicFile(i) => {
                let file_size: u64 = i.file_size.into();

                let mut offset: u64 = i.blocks_start.into();
                let mut remaining: u64 = file_size;
                for b in &i.block_sizes {
                    let data_len = if u64::from(block_size) <= remaining {
                        block_size
                    } else {
                        remaining as u32
                    };
                    let is_compressed = (b & 0x1000000) == 0;
                    let c = if is_compressed { comp } else { Compressor::None };
                    let disk_len = b & 0xFFFFFF;
                    let mut client_reader = inner.client();
                    client_reader.seek(SeekFrom::Start(offset))?;
                    blocks.push( FileBlockInfo {
                        disk_offset: offset,
                        disk_len,
                        data_offset: 0,
                        data_len,
                        is_compressed,
                        reader: BlockReader::Block(CachingReader::new_with_capacity(CompressedBlockReader::new(client_reader,c, disk_len.into(), data_len.into())?, block_size.try_into().unwrap())),
                    });
                    offset += u64::from(*b);
                    remaining -= u64::from(data_len);
                }
                if i.frag_index != u32::MAX {
                    let ft = metadata::FragmentLookupTable::read(&mut inner.client(), sb)?;
                    let f: &FragmentEntry = &ft.lu_table.entries[i.frag_index as usize];
                    blocks.push( FileBlockInfo {
                        disk_offset: f.start,
                        disk_len: f.size & 0xFFFFFF,
                        data_offset: i.block_offset,
                        data_len: i.file_size % block_size,
                        is_compressed: (f.size & 0x1000000) == 0,
                        // Note, block_size is not the uncompressed size; it is the maximum uncompressed size
                        reader: BlockReader::Fragment(frag_cache.get_fragment_reader(f.start, (f.size & 0xFFFFFF).into(), block_size.into(), i.block_offset.into(), (i.file_size % block_size).into())?),
                    });
                }
                Ok(Some(FileDataReader {
                    inner, pos, block_size, file_size, blocks, 
                }))
            }
            _ => Ok(None),
        }
    }

    pub fn into_inner(self) -> R {
        self.inner.into_inner()
    }

    /** Get the block and (uncompressed) data offset within the block for a given file offset.
     *  Also return the amount of (uncompressed) data left in the block
     */
    fn calc_block_and_offset(&mut self, pos: u64) -> Option<(&mut FileBlockInfo<R>, u32, u32)> {
        if pos >= self.file_size {
            return None;
        }
        let block_index = pos / (self.block_size as u64);
        let data_offset = (pos % (self.block_size as u64)) as u32;

        let b = &self.blocks[block_index as usize];
        let offset = data_offset;
        let remaining = b.data_len - data_offset;
        let b = &mut self.blocks[block_index as usize];
        Some((b, offset, remaining))
    }
}

impl<R: Read + Seek> Read for FileDataReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let (b, offset, _) = match self.calc_block_and_offset(self.pos) {
            Some(x) => x,
            None => return Ok(0),
        };
        b.reader.seek(SeekFrom::Start(offset.into()))?;
        let size = b.reader.read(buf)?;
        self.pos += u64::try_from(size).unwrap();
        Ok(size)
    }
}

impl<R: Read + Seek> Seek for FileDataReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match pos {
            SeekFrom::Start(p) => {
                self.pos = p;
                Ok(self.pos)
            },
            SeekFrom::End(p) => {
                assert!(self.file_size as i64 + p >= 0);
                self.pos = (self.file_size as i64 + p) as u64;
                Ok(self.pos)
            },
            SeekFrom::Current(p) => {
                assert!(self.pos as i64 + p >= 0);
                self.pos = (self.pos as i64 + p) as u64;
                Ok(self.pos)
            },
        }
    }
}