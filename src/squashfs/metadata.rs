use std::collections::HashMap;
use std::fmt::Debug;
use std::io::{self, Take};
use std::io::{Read, Seek, SeekFrom, BufReader};
use std::ffi::CString;
use std::mem;


use byteorder::{ByteOrder, LittleEndian, ReadBytesExt};
use flate2::read::ZlibDecoder;
use num_enum::{IntoPrimitive, TryFromPrimitive};

use super::superblock::{Compressor, Superblock};

//let block_count = (item_count + (METADATA_BLOCK_SIZE / I::BYTE_SIZE) as u32 - 1) / (METADATA_BLOCK_SIZE / I::BYTE_SIZE) as u32;

// Divide x by y, rounding up any fractional result
macro_rules! div_ceil {
    ($x:expr, $y:expr) => { ($x + $y - 1) / $y }
}

// SquashFS Metadata block size is fixed by the specification
const METADATA_BLOCK_SIZE: u16 = 8192;

// Read and decompress a single metadata block from the provided Reader into the provided buffer.
// Metadata blocks are always 8KB. If a smaller buffer is provided then only part of the block is
// read and the ending Reader position is undefined.
pub fn read_metadata_block<R>(r: &mut R, c: &Compressor, buf: &mut [u8]) -> io::Result<(usize, usize)>
    where R: Read
{
    let header: u16 = r.read_u16::<LittleEndian>()?;
    let size: u16 = header & 0x7FFF;
    let compressed: bool = header & 0x8000 == 0;

    //println!("Size = {}; Compressed = {}", size, compressed);

    if size > METADATA_BLOCK_SIZE {
        eprintln!("Metadata block size too big -- {}", size);
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }

    if !compressed {
        return r.read_exact(&mut buf[..size.into()]).map(|_| size.into()).map(|x| (size as usize +2, x));
    }

    match c {
        Compressor::Gzip => {
            let mut dec = ZlibDecoder::new(r.take(size.into()));
            let mut total = 0;
            while total < buf.len() {
                let read_size = dec.read(&mut buf[total..])?;
                total += read_size;
                if read_size == 0 {
                    break;
                }
            }
            Ok((size as usize +2,total))
        },
        _ => Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

// This is a reader that wraps the Reader for a backing-store of compressed metadata blocks,
// providing a more convenient interface for reading metadata. When the user first reads data from
// a new metadata block, the entire compressed block is immediately fully read and decompressed. To
// reduce repeated work, each decompressed block is placed in a cache which bypasses the
// read/decompress if the block is needed again in the future.
//
// Due to the SquashFS format using compressed offsets to specify metadata locations, the Reader
// position is handled in a non-standard way. Seek operations refer to compressed addresses, and
// it is expected that seeks will always refer to the beginning of a metadata block -- ie, since the
// reader has no way of knowing the boundaries of compressed metadata blocks, the user must
// provide them via the seek() call. seek() cannot currently be used to navigate within a metadata
// block -- unneeded data must be read and discarded.
//
// Once an initial metablock location has been established, the user may continue reading across
// multiple blocks with the CachingMetadataReader silently decompressing each new block as needed.
// If a user read is large enough to bridge across a block boundary then the Reader will return a
// short read up until the end of the current block, and the user must call read a second time to
// retrieve the next block's data.
#[derive(Debug)]
pub struct CachingMetadataReader<R> {
    inner: R,                                       // Backing reader on the compressed stream
    cur_pos: u64,                                   // Virtual seek offset within the *compressed* stream
    stream_pos: u64,                                // Actual seek position of backing reader
    comp: Compressor,                               // What compressor to use to decompress compressed blocks
    block_cache: HashMap<u64, (usize, Vec<u8>)>,    // Map of compressed block offsets to their uncompressed data
    cur_key: Option<u64>,                           // Key used to obtain currently possessed cache block
    cur_data: Option<(usize, Vec<u8>)>,             // Currently possessed cache block
    cur_offset: usize,                              // Read offset within possessed cache block
}

impl<R:Read + Seek> CachingMetadataReader<R> {
    pub fn new(mut inner: R, comp: Compressor) -> Self {
        let stream_pos = inner.stream_position().unwrap();
        let cur_pos = stream_pos;
        Self { inner, comp, block_cache: HashMap::new(), cur_key: None, cur_data: None, cur_offset: 0, cur_pos, stream_pos }
    }
}

impl<R: Read + Seek> Read for CachingMetadataReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cur_data.is_none() {
            let new_key = self.cur_pos;
            if let Some(d) = self.block_cache.remove(&new_key) {
                //eprintln!("Using cache ({})", new_key);
                self.cur_pos += d.0 as u64;
                self.cur_key = Some(new_key);
                self.cur_data = Some(d);
                self.cur_offset = 0;
            } else {
                //eprintln!("Using new read ({})", new_key);
                if self.cur_pos != self.stream_pos {
                    self.inner.seek(SeekFrom::Start(self.cur_pos))?;
                    self.stream_pos = self.cur_pos;
                }
                let mut buf: [u8;8192] = [0; 8192];
                let size = read_metadata_block(&mut self.inner, &self.comp, &mut buf)?;
                self.stream_pos += size.0 as u64;
                self.cur_pos += size.0 as u64;
                self.cur_key = Some(new_key);
                self.cur_data = Some((size.0, buf[..size.1].to_vec()));
                self.cur_offset = 0;
            }
        }

        if let Some(d) = &self.cur_data {
            let read_size = Read::read(&mut &d.1[self.cur_offset..], buf)?;
            self.cur_offset += read_size;

            //eprintln!("Read {}: Got {}; new offset {}/{}", buf.len(), read_size, self.cur_offset, d.1.len());

            if self.cur_offset >= d.1.len() {
                let cache_key = mem::take(&mut self.cur_key).unwrap();
                let cache_data = mem::take(&mut self.cur_data).unwrap();
                self.cur_offset = 0;
                self.block_cache.insert(cache_key, cache_data);
            }

            Ok(read_size)
        } else {
            Err(io::Error::from(io::ErrorKind::NotFound))
        }
    }
}

impl<R: Read + Seek> Seek for CachingMetadataReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        if self.cur_data.is_some() {
            // Deposit the current data block in the cache
            let cache_key = mem::take(&mut self.cur_key).unwrap();
            let cache_data = mem::take(&mut self.cur_data).unwrap();
            self.block_cache.insert(cache_key, cache_data);
        }
        //eprintln!("Seek: {:?}", pos);
        self.cur_pos = match pos {
            SeekFrom::Start(p) => p,
            SeekFrom::Current(p) => (self.cur_pos as i64 + p) as u64,
            _ => return Err(io::Error::from(io::ErrorKind::Unsupported))
        };
        Ok(self.cur_pos)
    }
}

enum InnerReader<R> {
    None,
    Base(R),
    Uncompressed(BufReader<Take<R>>),
    Gzip(ZlibDecoder<Take<R>>),
}

impl<R: Read> InnerReader<R> {
    fn take(&mut self) -> InnerReader<R> {
        mem::replace(self, InnerReader::None)
    }
}

impl<R: Read> Read for InnerReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            InnerReader::Base(r) => r.read(buf),
            InnerReader::Uncompressed(r) => r.read(buf),
            InnerReader::Gzip(r) => r.read(buf),
            _ => panic!("InnerReader::read: no inner value"),
        }
    }

}

pub struct MetadataReader<R> {
    inner: InnerReader<R>,
    comp: Compressor,
}

impl<R: Read + Seek> MetadataReader<R> {
    pub fn new(inner: R, comp: Compressor) -> MetadataReader<R> {
        MetadataReader { inner: InnerReader::Base(inner), comp }
    }

    pub fn into_inner(self) -> R {
        match self.inner {
            InnerReader::Base(r) => r,
            InnerReader::Uncompressed(r) => r.into_inner().into_inner(),
            InnerReader::Gzip(r) => r.into_inner().into_inner(),
            InnerReader::None => panic!("into_inner: no inner value"),
        }
    }

    fn start_block(&mut self) -> io::Result<()> {
        let mut inner = InnerReader::take(&mut self.inner);
        inner = match inner {
            InnerReader::Base(r) => InnerReader::Base(r),
            InnerReader::Uncompressed(r) => InnerReader::Base(r.into_inner().into_inner()),
            InnerReader::Gzip(r) => InnerReader::Base(r.into_inner().into_inner()),
            InnerReader::None => panic!("into_inner: no inner value"),
        };

        if let InnerReader::Base(mut r) = inner {
            let header: u16 = r.read_u16::<LittleEndian>()?;
            let size: u16 = header & 0x7FFF;
            let compressed: bool = header & 0x8000 == 0;
            //println!("MetadataReader: Starting block. Compressed = {}; Size = {}", compressed, size);

            self.inner = if !compressed {
                InnerReader::Uncompressed(BufReader::new(r.take(size.into())))
            } else {
                match self.comp {
                    Compressor::Gzip => InnerReader::Gzip(ZlibDecoder::new(r.take(size.into()))),
                    _ => { return Err(io::Error::from(io::ErrorKind::Unsupported)) },
                }
            };
            Ok(())
        } else {
            panic!("start_block: not Base reader");
        }
    }
}

impl<R: Read + Seek> Read for MetadataReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if let InnerReader::Base(_) = self.inner {
            self.start_block()?;
        }

        let mut size = self.inner.read(buf)?;
        if size == 0 && buf.len() != 0 {
            // This must be the end of the block. Try to start a new one.
            self.start_block()?;
            size = self.inner.read(buf)?;
        }
        Ok(size)
    }
}

impl<R: Read + Seek> Seek for MetadataReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let inner = InnerReader::take(&mut self.inner);
        self.inner = match inner {
            InnerReader::Base(r) => InnerReader::Base(r),
            InnerReader::Uncompressed(r) => InnerReader::Base(r.into_inner().into_inner()),
            InnerReader::Gzip(r) => InnerReader::Base(r.into_inner().into_inner()),
            InnerReader::None => panic!("into_inner: no inner value"),
        };
        if let InnerReader::Base(r) = &mut self.inner {
            r.seek(pos)
        } else {
            panic!("seek: not Base reader");
        }
    }
}

#[derive(Clone, Copy)]
pub struct EntryReference {
    val: u64
}

impl EntryReference {
    pub fn new(location: u64, offset: u16) -> Self {
        Self {
            val: (location << 16) | u64::from(offset),
        }
    }

    pub fn location(&self) -> u64 {
        self.val >> 16
    }

    pub fn offset(&self) -> u16 {
        (self.val & 0xFFFF) as u16
    }

    pub fn from_bytes(buf: &[u8]) -> Self {
        Self {
            val: LittleEndian::read_u64(buf)
        }
    }

    pub fn read<R>(r: &mut R) -> io::Result<Self>
    where R: Read
    {
        Ok(Self {
            val: r.read_u64::<LittleEndian>()?
        })
    }
}

impl std::fmt::Debug for EntryReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.location(), self.offset())
    }
}

#[derive(Debug)]
pub struct ExtendedAttribute {
    pub prefix: u16,
    pub name: String,
    pub value: AttributeValue,
}

#[derive(Debug)]
pub struct ExtendedAttributeLookupEntry {
    pub xattr_ref: u64,
    pub count: u32,
    pub size: u32,
}

#[derive(Debug)]
pub enum AttributeValue {
    Value(Vec<u8>),
    Ref(EntryReference),
}

pub trait FromBytes {
    const BYTE_SIZE: u16;
    fn from_bytes(buf: &[u8]) -> Self;
}

impl FromBytes for u32 {
    const BYTE_SIZE: u16 = std::mem::size_of::<Self>() as u16;
    fn from_bytes(buf: &[u8]) -> Self {
        u32::from_le_bytes(buf.try_into().unwrap())
    }
}

impl FromBytes for EntryReference {
    const BYTE_SIZE: u16 = std::mem::size_of::<Self>() as u16;
    fn from_bytes(buf: &[u8]) -> Self {
        Self {
            val: u64::from_le_bytes(buf.try_into().unwrap()),
        }
    }
}

impl FromBytes for ExtendedAttributeLookupEntry {
    const BYTE_SIZE: u16 = 16;
    fn from_bytes(buf: &[u8]) -> Self {
        Self {
            xattr_ref: u64::from_le_bytes(buf[0..8].try_into().unwrap()),
            count: u32::from_le_bytes(buf[8..12].try_into().unwrap()),
            size: u32::from_le_bytes(buf[12..16].try_into().unwrap()),
        }
    }
}

#[derive(Debug)]
pub struct LookupTable<I: FromBytes> {
    pub block_offsets: Vec<u64>,
    pub entries: Vec<I>,
}

impl<I: FromBytes> LookupTable<I> {
    fn read<R>(r: &mut R, table_offset: u64, item_count: u32, compressor: &Compressor) -> io::Result<Self>
    where R: Read + Seek
    {
        let block_count = div_ceil!(item_count, (METADATA_BLOCK_SIZE / I::BYTE_SIZE) as u32);
        let mut me = LookupTable {
            block_offsets: Vec::with_capacity(usize::try_from(block_count).unwrap()),
            entries: Vec::with_capacity(usize::try_from(item_count).unwrap()),
        };

        // Read the metadata block locations
        r.seek(SeekFrom::Start(table_offset))?;
        for _ in 0..block_count {
            me.block_offsets.push(r.read_u64::<LittleEndian>()?);
        }

        // Parse the metadata blocks into entries
        // TODO: Handle entries that cross block boundaries (is this possible?)
        let mut buf: [u8; 8192] = [0; 8192];
        let mut size = 0;
        let mut data = &buf[0..size];
        for n in 0..item_count {
            // Read the next block, if needed
            if n % (METADATA_BLOCK_SIZE / I::BYTE_SIZE) as u32 == 0 {
                let block_num = n / (METADATA_BLOCK_SIZE/I::BYTE_SIZE) as u32;
                r.seek(SeekFrom::Start(me.block_offsets[block_num as usize]))?;
                size = read_metadata_block(r, compressor, &mut buf)?.1;
                data = &buf[0..size];
            }

            me.entries.push(I::from_bytes(&data[..I::BYTE_SIZE as usize]));
            data = &data[I::BYTE_SIZE as usize..];
        }
        Ok(me)
    }
}

#[derive(Debug)]
pub struct FragmentEntry {
    pub start: u64,
    pub size: u32,
}

impl FromBytes for FragmentEntry {
    const BYTE_SIZE: u16 = 16;
    fn from_bytes(buf: &[u8]) -> Self {
        Self {
            start: u64::from_le_bytes(buf[0..8].try_into().unwrap()),
            size: u32::from_le_bytes(buf[8..12].try_into().unwrap()),
        }
    }
}

#[derive(Debug)]
pub struct FragmentLookupTable {
    pub lu_table: LookupTable<FragmentEntry>,
}

impl FragmentLookupTable {
    pub fn read<R>(r: &mut R, sb: &Superblock) -> io::Result<Self> 
    where R: Read + Seek
    {
        Ok(Self {
            lu_table: LookupTable::read(r, sb.frag_table, sb.frag_count as u32, &sb.compressor)?,
        })
    }
}

#[derive(Debug)]
pub struct IdLookupTable {
    pub lu_table: LookupTable<u32>,
}

impl IdLookupTable {
    pub fn read<R>(r: &mut R, sb: &Superblock) -> io::Result<Self> 
    where R: Read + Seek
    {
        Ok(Self {
            lu_table: LookupTable::read(r, sb.id_table, sb.id_count as u32, &sb.compressor)?,
        })
    }
}

#[derive(Debug)]
pub struct ExportLookupTable {
    pub lu_table: LookupTable<EntryReference>,
}

impl ExportLookupTable {
    pub fn read<R>(r: &mut R, sb: &Superblock) -> io::Result<Option<Self>> 
    where R: Read + Seek
    {
        if sb.export_table == u64::MAX {
            return Ok(None)
        }
        Ok(Some(Self {
            lu_table: LookupTable::read(r, sb.export_table, sb.inode_count, &sb.compressor)?,
        }))
    }
}

#[derive(Debug)]
pub struct ExtendedAttributeLookupTable {
    pub kv_start: u64,
    pub count: u32,
    pub lu_table: LookupTable<ExtendedAttributeLookupEntry>,
}

impl ExtendedAttributeLookupTable {
    pub fn read<R>(r: &mut R, sb: &Superblock) -> io::Result<Option<Self>> 
    where R: Read + Seek
    {
        if sb.xattr_table == u64::MAX {
            return Ok(None);
        }
        // Read the metadata block locations
        r.seek(SeekFrom::Start(sb.xattr_table))?;
        let kv_start = r.read_u64::<LittleEndian>()?;
        let count = r.read_u32::<LittleEndian>()?;
        Ok(Some(Self {
            kv_start, count,
            lu_table: LookupTable::read(r, sb.xattr_table+16, count as u32, &sb.compressor)?,
        }))
    }
}

#[derive(Debug)]
pub struct Inode {
    pub inode_type: InodeType,
    permissions: u16,
    uid_index: u16,
    gid_index: u16,
    mtime: u32,
    inode_number: u32,
    pub extended_info: InodeExtendedInfo,
}

#[derive(Debug, IntoPrimitive, TryFromPrimitive)]
#[repr(u16)]
pub enum InodeType {
    BasicDir = 1,
    BasicFile = 2,
    BasicSymlink = 3,
    BasicBlockDev = 4,
    BasicCharDev = 5,
    BasicNamedPipe = 6,
    BasicSocked = 7,
    ExtDir = 8,
    ExtFile = 9,
    ExtSymlink = 10,
    ExtBlockDev = 11,
    ExtCharDev = 12,
    ExtNamedPipe = 13,
    ExtSocked = 14,
    #[num_enum(default)]
    Unknown = 0xFFFF,
}

#[derive(Debug)]
pub enum InodeExtendedInfo {
    None,
    BasicDir(BasicDirInfo),
    ExtDir(ExtDirInfo),
    BasicFile(BasicFileInfo),
    BasicSymlink(BasicSymlinkInfo),
    BasicDev(BasicDevInfo),
    BasicIpc(BasicIpcInfo),
}

#[derive(Debug)]
pub struct BasicDirInfo {
    pub block_index: u32,
    pub link_count: u32,
    pub file_size: u16,
    pub block_offset: u16,
    pub parent_inode: u32,
}

#[derive(Debug)]
pub struct ExtDirInfo {
    pub link_count: u32,
    pub file_size: u32,
    pub block_index: u32,
    pub parent_inode: u32,
    pub index_count: u16,
    pub block_offset: u16,
    pub xattr_index: u32,
}

#[derive(Debug)]
pub struct BasicFileInfo {
    pub blocks_start: u32,
    pub frag_index: u32,
    pub block_offset: u32,
    pub file_size: u32,
    pub block_sizes: Vec<u32>,
}


#[derive(Debug)]
pub struct BasicSymlinkInfo {
    pub link_count: u32,
    pub target_path: CString,
}

#[derive(Debug)]
pub struct BasicDevInfo {
    pub link_count: u32,
    pub dev_number: u32,
}

#[derive(Debug)]
pub struct BasicIpcInfo {
    pub link_count: u32,
}

impl Inode {
    pub fn read<R>(r: &mut R, block_size: u32) -> io::Result<Self>
    where R: Read
    {
        let inode_type = InodeType::try_from(r.read_u16::<LittleEndian>()?).unwrap();
        let permissions = r.read_u16::<LittleEndian>()?;
        let uid_index = r.read_u16::<LittleEndian>()?;
        let gid_index = r.read_u16::<LittleEndian>()?;
        let mtime = r.read_u32::<LittleEndian>()?;
        let inode_number = r.read_u32::<LittleEndian>()?;
        let extended_info = match inode_type {
            InodeType::BasicDir => InodeExtendedInfo::BasicDir( BasicDirInfo {
                block_index : r.read_u32::<LittleEndian>()?,
                link_count : r.read_u32::<LittleEndian>()?,
                file_size : r.read_u16::<LittleEndian>()?,
                block_offset : r.read_u16::<LittleEndian>()?,
                parent_inode : r.read_u32::<LittleEndian>()?,
            }),
            InodeType::ExtDir => InodeExtendedInfo::ExtDir( ExtDirInfo {
                link_count : r.read_u32::<LittleEndian>()?,
                file_size : r.read_u32::<LittleEndian>()?,
                block_index : r.read_u32::<LittleEndian>()?,
                parent_inode : r.read_u32::<LittleEndian>()?,
                index_count : r.read_u16::<LittleEndian>()?,
                block_offset : r.read_u16::<LittleEndian>()?,
                xattr_index : r.read_u32::<LittleEndian>()?,
            }),
            InodeType::BasicFile => {
                let blocks_start = r.read_u32::<LittleEndian>()?;
                let frag_index = r.read_u32::<LittleEndian>()?;
                let block_offset = r.read_u32::<LittleEndian>()?;
                let file_size = r.read_u32::<LittleEndian>()?;
                let num_blocks = if frag_index == u32::MAX {
                    div_ceil!(file_size, block_size)
                } else {
                    file_size / block_size
                };
                let mut block_sizes = Vec::with_capacity(usize::try_from(num_blocks).unwrap());
                for _ in 0..num_blocks {
                    block_sizes.push(r.read_u32::<LittleEndian>()?);
                }

                InodeExtendedInfo::BasicFile( BasicFileInfo {
                    blocks_start, frag_index, block_offset, file_size, block_sizes
                })
            },
            InodeType::BasicSymlink => {
                let link_count = r.read_u32::<LittleEndian>()?;
                let target_size = r.read_u32::<LittleEndian>()?;
               
                let mut path_buf = Vec::with_capacity(usize::try_from(target_size).unwrap());
                let read_size = r.take(target_size.into()).read_to_end(&mut path_buf)?;
                if read_size != target_size.try_into().unwrap() {
                    return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
                }
                InodeExtendedInfo::BasicSymlink( BasicSymlinkInfo {
                    link_count,
                    target_path: CString::new(path_buf).unwrap(),
                })
            },
            InodeType::BasicCharDev => {
                InodeExtendedInfo::BasicDev( BasicDevInfo {
                    link_count : r.read_u32::<LittleEndian>()?,
                    dev_number : r.read_u32::<LittleEndian>()?,
                })

            },
            InodeType::BasicNamedPipe => {
                InodeExtendedInfo::BasicIpc( BasicIpcInfo {
                    link_count : r.read_u32::<LittleEndian>()?,
                })

            },
            _ => InodeExtendedInfo::None,

        };

        Ok(Self {
            inode_type, permissions, uid_index, gid_index, mtime, inode_number, extended_info 
        })
    }

    pub fn read_at_ref<R>(r: &mut R, sb: &Superblock, inode_ref: EntryReference) -> io::Result<Self>
    where R: Read + Seek
    {
        r.seek(SeekFrom::Start(sb.inode_table + inode_ref.location()))?;
        //let mut reader = MetadataReader::new(r, sb.compressor);
        io::copy(&mut r.by_ref().take(inode_ref.offset().into()), &mut io::sink())?;

        Self::read(r, sb.block_size)
    }

    pub fn from_bytes(buf: &mut [u8], block_size: u32) -> Self {
        Self::read(&mut &buf[..], block_size).unwrap()
    }

    pub fn is_dir(&self) -> bool {
        matches!(self.inode_type, InodeType::BasicDir) ||
        matches!(self.inode_type, InodeType::ExtDir)
    }

    pub fn inode_number(&self) -> u32 {
        self.inode_number
    }

    pub fn permissions(&self) -> u16 {
        self.permissions
    }

    pub fn mode(&self) -> u16 {
        let mut mode = self.permissions;
        mode |= match self.inode_type {
            InodeType::BasicBlockDev |
            InodeType::ExtBlockDev => 0o60000,
            InodeType::BasicCharDev |
            InodeType::ExtCharDev => 0o20000,
            InodeType::BasicDir |
            InodeType::ExtDir => 0o40000,
            InodeType::BasicFile |
            InodeType::ExtFile => 0o100000,
            InodeType::BasicNamedPipe |
            InodeType::ExtNamedPipe => 0o10000,
            InodeType::BasicSocked |
            InodeType::ExtSocked => 0o140000,
            InodeType::BasicSymlink |
            InodeType::ExtSymlink => 0o120000,
            InodeType::Unknown => 0,
        };
        mode
    }

    pub fn uid(&self, id_table: &IdLookupTable) -> io::Result<u32> {
        id_table.lu_table.entries.get(self.uid_index as usize).cloned()
        .ok_or(io::Error::from(io::ErrorKind::NotFound))
    }

    pub fn gid(&self, id_table: &IdLookupTable) -> io::Result<u32> {
        id_table.lu_table.entries.get(self.gid_index as usize).cloned()
        .ok_or(io::Error::from(io::ErrorKind::NotFound))
    }

    pub fn mtime(&self) -> u32 {
        self.mtime
    }
}

#[derive(Debug)]
pub struct DirTable {
    pub count: u32,
    pub start: u32,
    pub inode_number: u32,
    pub entries: Vec<DirEntry>,
}

#[derive(Debug)]
pub struct DirEntry {
    pub offset: u16,
    pub inode_offset: i16,
    pub inode_type: InodeType,
    pub name: CString,
}

impl DirTable {
    pub fn load<R>(r: &mut R) -> io::Result<Self>
    where R: Read
    {
        let count = r.read_u32::<LittleEndian>()? + 1;
        let start = r.read_u32::<LittleEndian>()?;
        let inode_number = r.read_u32::<LittleEndian>()?;
        let mut entries = Vec::with_capacity(count.try_into().unwrap());
        for _ in 0..count {
            let offset = r.read_u16::<LittleEndian>()?;
            let inode_offset = r.read_i16::<LittleEndian>()?;
            let inode_type = InodeType::try_from(r.read_u16::<LittleEndian>()?).unwrap();
            let name_size = r.read_u16::<LittleEndian>()? + 1;
            let mut name_buf = Vec::with_capacity(name_size.into());
            let read_size = r.take(name_size.into()).read_to_end(&mut name_buf)?;
            if read_size != name_size.into() {
                return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
            }
            entries.push(DirEntry {
                offset, inode_offset, inode_type,
                name: CString::new(name_buf).unwrap(),
            });
        }
        Ok(DirTable {
            count, start, inode_number, entries
        })
    }

    pub fn read_for_inode<R>(r: &mut R, sb: &Superblock, inode: &Inode) -> io::Result<Vec<Self>>
    where R: Read + Seek
    {
        let (block_index, block_offset, file_size) = match &inode.extended_info {
            InodeExtendedInfo::BasicDir(d) => (d.block_index, d.block_offset, d.file_size.into()),
            InodeExtendedInfo::ExtDir(d) => (d.block_index, d.block_offset, d.file_size),
            _ => {return Err(io::Error::new(io::ErrorKind::InvalidInput, "Inode is not a directory"))},
        };

        r.seek(SeekFrom::Start(sb.dir_table + block_index as u64))?;
        //let mut reader = MetadataReader::new(r, sb.compressor);
        io::copy(&mut r.by_ref().take(block_offset.into()), &mut io::sink())?;

        let mut reader = r.take((file_size-3).into());

        let mut tables = Vec::new();
        while reader.limit() > 0 {
            tables.push(Self::load(&mut reader)?);
        }
        Ok(tables)
    }
}