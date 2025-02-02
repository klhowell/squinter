use std::io;
use std::io::Read;

use bitflags::bitflags;
use byteorder::{LittleEndian, ReadBytesExt};
use num_enum::{IntoPrimitive, TryFromPrimitive};

use super::metadata::EntryReference;

#[allow(dead_code)]
pub const MAGIC: u32 = 0x73717368;

#[derive(Debug)]
pub struct Superblock {
    pub magic: u32,
    pub inode_count: u32,
    pub mod_time: u32,
    pub block_size: u32,
    pub frag_count: u32,
    pub compressor: Compressor,
    pub block_log: u16,
    pub flags: SuperblockFlags,
    pub id_count: u16,
    pub version_major: u16,
    pub version_minor: u16,
    pub root_inode: EntryReference,
    pub bytes_used: u64,
    pub id_table: u64,
    pub xattr_table: u64,
    pub inode_table: u64,
    pub dir_table: u64,
    pub frag_table: u64,
    pub export_table: u64,
}

bitflags! {
    pub struct SuperblockFlags: u16 {
        const INODES_UNCOMPRESSED = 0x0001;
        const DATABLOCKS_UNCOMPRESSED = 0x0002;
        const FRAGMENTS_UNCOMPRESSED = 0x0008;
        const FRAGMENTS_NOT_USED = 0x0010;
        const FRAGMENTS_ALWAYS_GENERATED = 0x0020;
        const DATA_DEDUPLICATED = 0x0040;
        const NFS_EXPORT_EXISTS = 0x0080;
        const XATTRS_UNCOMPRESSED = 0x0100;
        const NO_XATTRS = 0x0200;
        const COMPRESSOR_OPTIONS_PRESENT = 0x0400;
        const ID_TABLE_UNCOMPRESSED = 0x0800;
    }
}

#[derive(Debug, IntoPrimitive, TryFromPrimitive, Clone, Copy)]
#[repr(u16)]
pub enum Compressor {
    Gzip = 1,
    Lzo = 2,
    Lzma = 3,
    Xz = 4,
    Lz4 = 5,
    Zstd = 6,
    #[num_enum(default)]
    Unknown = 0xFFFF,
}

#[allow(dead_code)]
pub struct GzipOptions {
    pub compression_level: u32,
    pub window_size: u16,
    pub strategies: GzipStrategies,
}

bitflags! {
    pub struct GzipStrategies: u16 {
        const DEFAULT  = 0x0001;
        const FILTERED = 0x0002;
        const HUFFMAN_ONLY = 0x0004;
        const RLE = 0x0008;
        const FIXED = 0x0010;
    }
}


impl Superblock {
    pub fn read<R>(r: &mut R) -> io::Result<Superblock>
    where R: Read
    {
        Ok(Superblock {
            magic: r.read_u32::<LittleEndian>()?,
            inode_count: r.read_u32::<LittleEndian>()?,
            mod_time: r.read_u32::<LittleEndian>()?,
            block_size: r.read_u32::<LittleEndian>()?,
            frag_count: r.read_u32::<LittleEndian>()?,
            compressor: Compressor::try_from(r.read_u16::<LittleEndian>()?).unwrap(),
            block_log: r.read_u16::<LittleEndian>()?,
            flags: SuperblockFlags::from_bits_truncate(r.read_u16::<LittleEndian>()?),
            id_count: r.read_u16::<LittleEndian>()?,
            version_major: r.read_u16::<LittleEndian>()?,
            version_minor: r.read_u16::<LittleEndian>()?,
            root_inode: EntryReference::read(r)?,
            bytes_used: r.read_u64::<LittleEndian>()?,
            id_table: r.read_u64::<LittleEndian>()?,
            xattr_table: r.read_u64::<LittleEndian>()?,
            inode_table: r.read_u64::<LittleEndian>()?,
            dir_table: r.read_u64::<LittleEndian>()?,
            frag_table: r.read_u64::<LittleEndian>()?,
            export_table: r.read_u64::<LittleEndian>()?,
        })
    }

    pub fn from_bytes(b: &[u8]) -> io::Result<Superblock>
    {
        Superblock::read(&mut &b[..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[test]
    #[ignore = "requires manually provided squashfs"]
    fn test_read_superblock() -> io::Result<()> {
        let mut f = File::open("test_data/1.squashfs")?;

        let sb = Superblock::read(&mut f)?;
        assert_eq!(sb.magic, MAGIC);
        println!("Superblock = {:?}", sb);
        Ok(())
    }

}