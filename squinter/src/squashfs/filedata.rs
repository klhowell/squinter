use std::io;
use std::io::{Read, Seek, SeekFrom};

use super::block::{CachingReader, FragmentBlockCache, FragmentReader};
use super::metadata::{self, FragmentEntry, Inode, InodeExtendedInfo};
use super::readermux::ReaderClient;
use super::superblock::{Superblock, Compressor};
use super::compressed::CompressedBlockReader;

#[derive(PartialEq)]
struct FileBlockInfo<R> {
    disk_offset: u64,
    disk_len: u32,
    data_offset: u32,       // How far into this block the file data starts (only applies to tail-ends)
    data_len: u32,          // Amount of uncompressed file data in this block
    is_compressed: bool,
    reader: Option<R>,
}

// TODO: TailEnd support
#[allow(dead_code)]
struct TailEnd {
    fragment: FragmentEntry,
    offset: u32,
}

/// Reader for uncompressed file data from a series of compressed blocks and possibly a tail-end.
///
/// This reader presents a single contiguous stream to the caller. Internally, it cannot seek to
/// specific location within compressed blocks. Rather, it knows the starting location and size of
/// each compressed block and how much total uncompressed data that block contains. To seek into
/// a block, the reader must seek the base reader to the beginning of the compressed block, and then
/// use the decompressor to advance to the desired location within the block.
/// 
/// The stream position can be calculated from file position of the current block + the amount
/// already read from that block.
pub struct FileDataReader<R: Read + Seek> {
    inner: CompressedBlockReader<R>,
    pos: u64,
    comp: Compressor,
    block_size: u32,
    file_size: u64,
    blocks: Vec<FileBlockInfo<FragmentReader<ReaderClient<CachingReader<CompressedBlockReader<ReaderClient<R>>>>>>>,
}

impl<R: Read + Seek> FileDataReader<R> {
    pub fn from_inode<MDR>(inner: R, md_reader: &mut MDR, sb: &Superblock, frag_cache: &mut FragmentBlockCache<R>, inode: &Inode) -> io::Result<Option<Self>>
    where MDR: Read + Seek
    {
        let pos = 0;
        let comp = sb.compressor;
        let block_size = sb.block_size;
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
                    blocks.push( FileBlockInfo {
                        disk_offset: offset,
                        disk_len: b & 0xFFFFFF,
                        data_offset: 0,
                        data_len: data_len,
                        is_compressed: (b & 0x1000000) == 0,
                        reader: None,
                    });
                    offset += u64::from(*b);
                    remaining -= u64::from(data_len);
                }
                if i.frag_index != u32::MAX {
                    let ft = metadata::FragmentLookupTable::read(md_reader, sb)?;
                    let f: &FragmentEntry = &ft.lu_table.entries[i.frag_index as usize];
                    blocks.push( FileBlockInfo {
                        disk_offset: f.start,
                        disk_len: f.size & 0xFFFFFF,
                        data_offset: i.block_offset,
                        data_len: i.file_size % block_size,
                        is_compressed: (f.size & 0x1000000) == 0,
                        // Note, block_size is not the uncompressed size; it is the maximum uncompressed size
                        reader: Some(frag_cache.get_fragment_reader(f.start, (f.size & 0xFFFFFF).into(), block_size.into(), i.block_offset.into(), (i.file_size & block_size).into())?)
                    });
                }
                Ok(Some(FileDataReader {
                    inner: CompressedBlockReader::Base(inner), pos, comp, block_size, file_size, blocks, 
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
    fn calc_block_and_offset(&self, pos: u64) -> Option<(&FileBlockInfo<FragmentReader<ReaderClient<CachingReader<CompressedBlockReader<ReaderClient<R>>>>>>, u32, u32)> {
        if pos >= self.file_size {
            return None;
        }
        let block_index = pos / (self.block_size as u64);
        let data_offset = (pos % (self.block_size as u64)) as u32;

        let b = &self.blocks[block_index as usize];
        Some((b, b.data_offset + data_offset, b.data_len - data_offset))
    }

    /// Do whatever is necessary to make the inner reader's next read come from self.pos
    fn prepare_inner(&mut self) -> io::Result<()> {
        // First close any existing block reader
        let inner = CompressedBlockReader::take(&mut self.inner);
        let inner = inner.into_base();

        // Figure out which block to open
        let (block, offset, data_remaining) = self.calc_block_and_offset(self.pos).ok_or(io::Error::from(io::ErrorKind::UnexpectedEof))?;
        //eprintln!("Block Offset={}  Remaining={}", offset, data_remaining);

        // Open a reader on the block and move to the correct inner offset
        if let CompressedBlockReader::Base(mut r) = inner {
            self.inner = if !block.is_compressed {
                // For uncompressed blocks, we can directly seek to the target offset
                r.seek(SeekFrom::Start(block.disk_offset + u64::from(offset)))?;
                CompressedBlockReader::Uncompressed(r.take(data_remaining.into()))
            } else {
                // For compressed blocks, we must seek to the beginning of the block and then
                // read/discard to the target offset.
                r.seek(SeekFrom::Start(block.disk_offset))?;
                let mut inner = CompressedBlockReader::new(r, self.comp, block.disk_len.into(), (offset+data_remaining).into())?;
                io::copy(&mut Read::take(&mut inner, offset.into()), &mut io::sink())?;
                inner
            };
            Ok(())
        } else {
            panic!("start_block: not Base reader");
        }
    }
}

impl<R: Read + Seek> Read for FileDataReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.file_size {
            return Ok(0);
        }
        if let CompressedBlockReader::Base(_) = self.inner {
            self.prepare_inner()?;
        }

        let mut size = self.inner.read(buf)?;
        if size == 0 && buf.len() != 0 {
            // This must be the end of the block. Try to start a new one.
            self.prepare_inner()?;
            size = self.inner.read(buf)?;
        }
        self.pos += u64::try_from(size).unwrap();
        Ok(size)
    }
}

impl<R: Read + Seek> Seek for FileDataReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target_pos = match pos {
            SeekFrom::Start(x) => Ok(x),
            SeekFrom::End(x) => u64::try_from(x + i64::try_from(self.file_size).unwrap()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput)),
            SeekFrom::Current(x) => u64::try_from(x + i64::try_from(self.pos).unwrap()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput)),
        }?;

        if matches!(self.inner, CompressedBlockReader::Base(_)) {
            self.pos = target_pos;
            return Ok(self.pos);
        }

        // If we're able to directly navigate to the desired location within the current reader, do so
        // Otherwise, Retrieve the base reader, seek it, and read into the new block
        // Calculate the block and in-block position desired
        let (cur_block, cur_data_offset, _) = self.calc_block_and_offset(self.pos).unwrap();
        let (target_block, target_data_offset, _) = self.calc_block_and_offset(target_pos).unwrap();

        if std::ptr::eq(cur_block,target_block) && cur_data_offset <= target_data_offset {
            // Seeking later in the same block. No need to restart the block. Just read ahead.
            // TODO: For uncompressed blocks, a fresh seek is probably faster
            let distance = target_data_offset - cur_data_offset;
            io::copy(&mut Read::take(&mut self.inner, distance.into()), &mut io::sink())?;
            self.pos = target_pos;
        } else {
            self.pos = target_pos;
            self.prepare_inner()?;
        }
        Ok(self.pos)
    }
}