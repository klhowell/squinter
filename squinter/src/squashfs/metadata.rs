use std::cmp::min;
use std::fmt::Debug;
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::ffi::CString;
use std::ops::Range;

use byteorder::{ByteOrder, LittleEndian, ReadBytesExt};
use num_enum::{IntoPrimitive, TryFromPrimitive};

use super::block::{MetadataBlockCache, MetadataReader};
use super::compressed::CompressedBlockReader;
use super::superblock::{Compressor, Superblock};

//let block_count = (item_count + (METADATA_BLOCK_SIZE / I::BYTE_SIZE) as u32 - 1) / (METADATA_BLOCK_SIZE / I::BYTE_SIZE) as u32;

// Divide x by y, rounding up any fractional result
macro_rules! div_ceil {
    ($x:expr, $y:expr) => { ($x + $y - 1) / $y }
}

// SquashFS Metadata block size is fixed by the specification
const METADATA_BLOCK_SIZE: u16 = 8192;

/// Read and decompress a single metadata block from the provided Reader into the provided buffer.
/// Metadata blocks are always 8KB. If a smaller buffer is provided then only part of the block is
/// read and the ending Reader position is undefined.
pub(crate) fn read_metadata_block<R>(r: &mut R, c: &Compressor, buf: &mut [u8]) -> io::Result<(usize, usize)>
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

    // Set up a bounds so that we won't read more source data that the size or more uncompressed
    // data than the target size
    let max_read = min(buf.len(), METADATA_BLOCK_SIZE.into());
    let mut reader = CompressedBlockReader::new(r, *c, size.into(), max_read as u64)?;
    let mut writer = Cursor::new(buf);
    let total = io::copy(&mut reader, &mut writer)?;

    Ok(((size as usize) + 2, total as usize))
}

#[derive(Debug)]
pub struct MetadataProvider<R: Read+Seek> {
    cache: MetadataBlockCache<R>,
    inode_addrs: Range<u64>,
    dir_addrs: Range<u64>,
    frag_addrs: Range<u64>,
    export_addrs: Range<u64>,
    id_addrs: Range<u64>,
    xattr_addrs: Option<Range<u64>>,
}

impl<R:Read+Seek> MetadataProvider<R> {
    pub fn new(inner: R, sb: &Superblock) -> Self {
        let cache = MetadataBlockCache::new(inner, sb.compressor);
        let inode_addrs = sb.inode_table..sb.dir_table;
        let dir_addrs = if sb.frag_table != u64::MAX {
            sb.dir_table..sb.frag_table
        } else if sb.export_table != u64::MAX {
            sb.dir_table..sb.export_table
        } else {
            sb.dir_table..sb.id_table
        };
        let frag_addrs = if sb.export_table != u64::MAX {
            sb.frag_table..sb.export_table
        } else {
            sb.dir_table..sb.id_table
        };
        let export_addrs = sb.export_table..sb.id_table;
        let id_addrs = if sb.xattr_table != u64::MAX {
            sb.id_table..sb.xattr_table
        } else {
            sb.id_table..sb.bytes_used
        };
        let xattr_addrs = if sb.xattr_table != u64::MAX {
            Some(sb.xattr_table..sb.bytes_used)
        } else {
            None
        };
        MetadataProvider { cache, inode_addrs, dir_addrs, frag_addrs, export_addrs, id_addrs, xattr_addrs }
    }
    
    pub fn archive_reader(&self, entry_ref: EntryReference) -> io::Result<MetadataReader<R>> {
        MetadataReader::new(
            &self.cache,
            0,
            None,
            entry_ref,
        )
    }

    pub fn inode_reader(&self, entry_ref: EntryReference) -> io::Result<MetadataReader<R>> {
        MetadataReader::new(
            &self.cache,
            self.inode_addrs.start,
            Some(self.inode_addrs.end),
            entry_ref,
        )
    }

    pub fn dir_reader(&self, entry_ref: EntryReference) -> io::Result<MetadataReader<R>> {
        MetadataReader::new(
            &self.cache,
            self.dir_addrs.start,
            Some(self.dir_addrs.end),
            entry_ref,
        )
    }
}

/// An opaque reference value that can be used to retrieve a specific [`Inode`] using [`inode_from_entryref`].
///
/// [`Inode`]: struct.Inode.html
/// [`inode_from_entryref`]: ../squashfs/struct.SquashFS.html#method.inode_from_entryref
#[derive(Clone, Copy)]
pub struct EntryReference {
    val: u64
}

#[allow(dead_code)]
impl EntryReference {
    pub(crate) fn new(location: u64, offset: u16) -> Self {
        Self {
            val: (location << 16) | u64::from(offset),
        }
    }

    pub(crate) fn location(&self) -> u64 {
        self.val >> 16
    }

    pub(crate) fn offset(&self) -> u16 {
        (self.val & 0xFFFF) as u16
    }

    pub(crate) fn from_bytes(buf: &[u8]) -> Self {
        Self {
            val: LittleEndian::read_u64(buf)
        }
    }

    pub(crate) fn read<R>(r: &mut R) -> io::Result<Self>
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

#[allow(dead_code)]
#[derive(Debug)]
struct ExtendedAttribute {
    pub prefix: u16,
    pub name: String,
    pub value: AttributeValue,
}

#[allow(dead_code)]
#[derive(Debug)]
struct ExtendedAttributeLookupEntry {
    pub xattr_ref: u64,
    pub count: u32,
    pub size: u32,
}

#[allow(dead_code)]
#[derive(Debug)]
enum AttributeValue {
    Value(Vec<u8>),
    Ref(EntryReference),
}

pub(crate) trait FromBytes {
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
pub(crate) struct LookupTable<I: FromBytes> {
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
    
    fn read_one<R,P>(r: &mut R, mp: &MetadataProvider<P>, table_offset: u64, index: usize) -> io::Result<I>
    where R: Read + Seek,
          P: Read + Seek,
    {
        let meta_index = index * I::BYTE_SIZE as usize / METADATA_BLOCK_SIZE as usize;
        let block_offset: u16 = ((index * I::BYTE_SIZE as usize) % METADATA_BLOCK_SIZE as usize).try_into().unwrap();

        // Read the metadata block location
        r.seek(SeekFrom::Start(table_offset + (meta_index as u64 * 8)))?;
        let block_addr = r.read_u64::<LittleEndian>()?;

        let mut reader = mp.archive_reader(EntryReference::new(block_addr, block_offset))?;
        let mut buf = vec![0; I::BYTE_SIZE as usize];
        reader.read_exact(&mut buf)?;
        Ok(I::from_bytes(&buf))
    }
}

#[derive(Debug)]
pub(crate) struct FragmentEntry {
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
pub(crate) struct FragmentLookupTable {
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
    
    pub fn read_one<R,P>(r: &mut R, mp: &MetadataProvider<P>, sb: &Superblock, index: usize) -> io::Result<FragmentEntry>
    where R: Read + Seek,
          P: Read + Seek,
    {
        LookupTable::read_one(r, mp, sb.frag_table, index)
    }
}

#[derive(Debug)]
pub(crate) struct IdLookupTable {
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

#[allow(dead_code)]
#[derive(Debug)]
struct ExportLookupTable {
    pub lu_table: LookupTable<EntryReference>,
}

impl ExportLookupTable {
    #[allow(dead_code)]
    fn read<R>(r: &mut R, sb: &Superblock) -> io::Result<Option<Self>>
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

#[allow(dead_code)]
#[derive(Debug)]
struct ExtendedAttributeLookupTable {
    kv_start: u64,
    count: u32,
    lu_table: LookupTable<ExtendedAttributeLookupEntry>,
}

impl ExtendedAttributeLookupTable {
    #[allow(dead_code)]
    fn read<R>(r: &mut R, sb: &Superblock) -> io::Result<Option<Self>>
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

/// Information about an object in the filesystem (ex. file, directory, device node)
#[derive(Debug)]
pub struct Inode {
    inode_type: InodeType,
    permissions: u16,
    uid_index: u16,
    gid_index: u16,
    mtime: u32,
    inode_number: u32,
    pub extended_info: InodeExtendedInfo,
}

#[derive(Debug, IntoPrimitive, TryFromPrimitive)]
#[repr(u16)]
enum InodeType {
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

/// Type-specific information about a filesystem object
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

/// Information about a directory object in the filesystem
#[allow(dead_code)]
#[derive(Debug)]
pub struct BasicDirInfo {
    block_index: u32,
    link_count: u32,
    file_size: u16,
    block_offset: u16,
    pub parent_inode: u32,
}

/// Information about a directory object with extended attributes in the filesystem
#[allow(dead_code)]
#[derive(Debug)]
pub struct ExtDirInfo {
    link_count: u32,
    pub file_size: u32,
    block_index: u32,
    pub parent_inode: u32,
    index_count: u16,
    block_offset: u16,
    xattr_index: u32,
}

/// Information about a file object in the filesystem
#[allow(dead_code)]
#[derive(Debug)]
pub struct BasicFileInfo {
    pub(crate) blocks_start: u32,
    pub(crate) frag_index: u32,
    pub(crate) block_offset: u32,
    pub file_size: u32,
    pub(crate) block_sizes: Vec<u32>,
}


/// Information about a symbolic link object in the filesystem
#[allow(dead_code)]
#[derive(Debug)]
pub struct BasicSymlinkInfo {
    link_count: u32,
    pub target_path: CString,
}

/// Information about a device object in the filesystem
#[allow(dead_code)]
#[derive(Debug)]
pub struct BasicDevInfo {
    link_count: u32,
    pub dev_number: u32,
}

/// Information about an IPC object in the filesystem
#[allow(dead_code)]
#[derive(Debug)]
pub struct BasicIpcInfo {
    link_count: u32,
}

impl Inode {
    #[allow(dead_code)]
    pub(crate) fn read<R>(r: &mut R, block_size: u32) -> io::Result<Self>
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

    #[allow(dead_code)]
    pub(crate) fn read_at_ref<R>(r: &mut R, sb: &Superblock, inode_ref: EntryReference) -> io::Result<Self>
    where R: Read + Seek
    {
        r.seek(SeekFrom::Start(sb.inode_table + inode_ref.location()))?;
        //let mut reader = MetadataReader::new(r, sb.compressor);
        io::copy(&mut r.by_ref().take(inode_ref.offset().into()), &mut io::sink())?;

        Self::read(r, sb.block_size)
    }

    #[allow(dead_code)]
    pub(crate) fn from_bytes(buf: &mut [u8], block_size: u32) -> Self {
        Self::read(&mut &buf[..], block_size).unwrap()
    }

    pub fn is_file(&self) -> bool {
        matches!(self.inode_type, InodeType::BasicFile) ||
        matches!(self.inode_type, InodeType::ExtFile)
    }

    pub fn is_dir(&self) -> bool {
        matches!(self.inode_type, InodeType::BasicDir) ||
        matches!(self.inode_type, InodeType::ExtDir)
    }

    pub fn is_symlink(&self) -> bool {
        matches!(self.inode_type, InodeType::BasicSymlink) ||
        matches!(self.inode_type, InodeType::ExtSymlink)
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

    pub fn uid<R:Read+Seek>(&self, sqfs: &super::SquashFS<R>) -> io::Result<u32> {
        let id_table = &sqfs.id_table;
        id_table.lu_table.entries.get(self.uid_index as usize).cloned()
        .ok_or(io::Error::from(io::ErrorKind::NotFound))
    }

    pub fn gid<R:Read+Seek>(&self, sqfs: &super::SquashFS<R>) -> io::Result<u32> {
        let id_table = &sqfs.id_table;
        id_table.lu_table.entries.get(self.gid_index as usize).cloned()
        .ok_or(io::Error::from(io::ErrorKind::NotFound))
    }

    pub fn file_size(&self) -> Option<u32> {
        match &self.extended_info {
            InodeExtendedInfo::BasicFile(i) => Some(i.file_size),
            InodeExtendedInfo::BasicDir(i) => Some(i.file_size as u32),
            InodeExtendedInfo::ExtDir(i) => Some(i.file_size),
            _ => None,
        }
    }

    pub fn mtime(&self) -> u32 {
        self.mtime
    }
}

#[doc(hidden)]
#[derive(Debug)]
pub struct DirTable {
    pub count: u32,
    pub start: u32,
    pub inode_number: u32,
    pub entries: Vec<DirEntry>,
}

#[doc(hidden)]
#[allow(dead_code)]
#[derive(Debug)]
pub struct DirEntry {
    pub(crate) offset: u16,
    pub(crate) inode_offset: i16,
    inode_type: InodeType,
    pub(crate) name: CString,
}

impl DirTable {
    pub(crate) fn load<R>(r: &mut R) -> io::Result<Self>
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

    pub(crate) fn entryref_from_inode(inode: &Inode) -> io::Result<EntryReference> {
        let (block_addr, block_offset) = match &inode.extended_info {
            InodeExtendedInfo::BasicDir(d) => (d.block_index, d.block_offset),
            InodeExtendedInfo::ExtDir(d) => (d.block_index, d.block_offset),
            _ => {return Err(io::Error::new(io::ErrorKind::InvalidInput, "Inode is not a directory"))},
        };
        Ok(EntryReference::new(block_addr.into(), block_offset))
    }

    pub(crate) fn read_for_inode<R>(r: &mut MetadataReader<R>, inode: &Inode) -> io::Result<Vec<Self>>
    where R: Read + Seek
    {
        let (block_index, block_offset, file_size) = match &inode.extended_info {
            InodeExtendedInfo::BasicDir(d) => (d.block_index, d.block_offset, d.file_size.into()),
            InodeExtendedInfo::ExtDir(d) => (d.block_index, d.block_offset, d.file_size),
            _ => {return Err(io::Error::new(io::ErrorKind::InvalidInput, "Inode is not a directory"))},
        };

        r.seek_ref(EntryReference::new(block_index.into(), block_offset))?;
        let mut reader = r.take((file_size-3).into());

        let mut tables = Vec::new();
        while reader.limit() > 0 {
            tables.push(Self::load(&mut reader)?);
        }
        Ok(tables)
    }
}
