//! See https://dr-emann.github.io/squashfs/squashfs.html
//! for details on the SquashFS binary format

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Component, Path};
use std::boxed::Box;

use super::filedata::FileDataReader;
use super::metadata::{self, EntryReference, Inode, MetadataProvider};
use super::block::{FragmentBlockCache, MetadataReader};
use super::readermux::{ReaderMux, ReaderClient};
use super::superblock::Superblock;

/// The top-level interface to a SquashFS filesystem. This struct can be used to look up Inodes,
/// list directory contents, and open file data readers.
#[derive(Debug)]
pub struct SquashFS<R:Read+Seek> {
    reader_mux: Box<ReaderMux<R>>,
    frag_cache: FragmentBlockCache<ReaderClient<R>>,
    md_provider: MetadataProvider<ReaderClient<R>>,
    sb: Superblock,
    pub(crate) id_table: metadata::IdLookupTable,
}

impl SquashFS<BufReader<File>> {
    /// Open the contents of a filepath as a SquashFS, using a BufReader
    pub fn open<P>(path: P) -> io::Result<Self>
    where P: AsRef<Path>
    {
        Self::new(BufReader::new(File::open(path)?))
    }
}

impl<R: Read + Seek> SquashFS<R> {
    /// Create a new SquashFS instance from the provided Reader
    pub fn new(mut r: R) -> io::Result<Self>
    {
        r.seek(SeekFrom::Start(0))?;
        let sb = Superblock::read(&mut r)?;
        let id_table = metadata::IdLookupTable::read(&mut r, &sb)?;
        let mut reader_mux = Box::new(ReaderMux::new(r));
        let frag_cache = FragmentBlockCache::new(reader_mux.client(), sb.compressor);
        let md_provider = MetadataProvider::new(reader_mux.client(), &sb);
        Ok(SquashFS { reader_mux, frag_cache, md_provider, sb, id_table })
    }

    /// Retrieve an iterator that walks the dirents within a directory specified by the given
    /// path. path must refer to an existing directory or this function returns an error.
    pub fn read_dir<P>(&mut self, path: P) -> io::Result<ReadDir<std::vec::IntoIter<metadata::DirTable>>>
    where P: AsRef<Path>
    {
        let inode = self.inode_from_path(path)?;
        self.read_dir_inode(&inode)
    }

    /// Retrieve an iterator that walks the dirents within a directory specified by the given
    /// DirEntry.
    pub fn read_dir_dirent(&mut self, dir_entry: &DirEntry) -> io::Result<ReadDir<std::vec::IntoIter<metadata::DirTable>>>
    {
        let inode = self.inode_from_entryref(dir_entry.inode_ref)?;
        self.read_dir_inode(&inode)
    }

    /// Retrieve an iterator that walks the dirents within a directory specified by the given
    /// Inode.
    pub fn read_dir_inode(&mut self, inode: &metadata::Inode) -> io::Result<ReadDir<std::vec::IntoIter<metadata::DirTable>>>
    {
        // TODO: This method has some redundancy. Look at refactoring read_for_inode
        let entry_ref = metadata::DirTable::entryref_from_inode(inode)?;
        let mut reader = self.md_provider.dir_reader(entry_ref)?;
        let dir_tables = metadata::DirTable::read_for_inode(&mut reader, &inode)?;
        Ok(ReadDir::new(dir_tables.into_iter()))
    }

    /// Create an IO reader for the contents of the file specified by the given path
    pub fn open_file<P>(&mut self, path: P) -> io::Result<FileDataReader<ReaderClient<R>>>
    where P: AsRef<Path>
    {
        let inode = self.inode_from_path(path)?;
        self.open_file_inode(&inode)
    }

    /// Create an IO reader for the contents of the file specified by the given DirEntry
    pub fn open_file_dirent<P>(&mut self, dir_entry: &DirEntry) -> io::Result<FileDataReader<ReaderClient<R>>>
    {
        let inode = self.inode_from_entryref(dir_entry.inode_ref)?;
        self.open_file_inode(&inode)
    }

    /// Create an IO reader for the contents of the file specified by the given Inode
    pub fn open_file_inode(&mut self, inode: &metadata::Inode) -> io::Result<FileDataReader<ReaderClient<R>>> {
        let reader = self.reader_mux.client();
        Ok(FileDataReader::from_inode(reader, &self.md_provider, &self.sb, &mut self.frag_cache, inode)?.unwrap())
    }

    /// Retrieve the root Inode of the SquashFS. This corresponds to the '/' directory
    pub fn root_inode(&mut self) -> io::Result<metadata::Inode> {
        let mut reader = self.md_provider.inode_reader(self.sb.root_inode)?;
        metadata::Inode::read(&mut reader, self.sb.block_size)
    }

    /// Retrieve the Inode specified by SquashFS metadata Entry Reference
    pub fn inode_from_entryref(&mut self, inode_ref: metadata::EntryReference) -> io::Result<metadata::Inode> {
        let mut reader = self.md_provider.inode_reader(inode_ref)?;
        metadata::Inode::read(&mut reader, self.sb.block_size)
    }

    /// Retreive the Inode specified by the given path
    pub fn inode_from_path<P>(&mut self, path: P) -> io::Result<metadata::Inode>
    where P: AsRef<Path>
    {
        let mut inode = self.root_inode()?;
        for comp in path.as_ref().components() {
            inode = match comp {
                Component::RootDir => self.root_inode()?,
                Component::Normal(n) => {
                    self.read_dir_inode(&inode)?.find(|e| n == e.file_name().as_str())
                        .map(|e| self.inode_from_entryref(e.inode_ref()).ok())
                        .flatten()
                        .ok_or(io::Error::from(io::ErrorKind::NotFound))?
                },
                _ => { return Err(io::Error::new(io::ErrorKind::InvalidData, "Error parsing filepath"))}
            };
        }
        Ok(inode)
    }
}

/// A DirEntry, like in std::fs, represents a named inode-reference within a directory. For example, a filename
/// together with a reference to the file's inode.
#[derive(Debug)]
pub struct DirEntry {
    inner: metadata::DirEntry,
    inode_ref: metadata::EntryReference,
    inode_num: u32,
}

impl DirEntry {
    fn new(dt_start: u64, dt_inode_num: u32, inner: metadata::DirEntry) -> Self {
        let inode_ref = metadata::EntryReference::new(dt_start, inner.offset);
        let inode_num = dt_inode_num.wrapping_add_signed(inner.inode_offset as i32);
        Self { inner, inode_ref, inode_num }
    }

    pub fn file_name(&self) -> String {
        self.inner.name.to_str().unwrap().to_string()
    }

    pub fn inode_ref(&self) -> metadata::EntryReference {
        self.inode_ref
    }

    #[allow(dead_code)]
    fn inode_num(&self) -> u32 {
        self.inode_num
    }
}

/// An iterator over the individual DirEntries in a directory
// This iterator combines DirEntries from a list of DirEntry runs. The SquashFS filesystem
// potentially splits the DirEntries for a directory table across multiple runs, so this object must
// combine walking of both runs and entries into the next() Iterator function.
#[derive(Debug)]
pub struct ReadDir<TI> {
    table_iter: TI,
    cur_iter: Option<std::vec::IntoIter<metadata::DirEntry>>,
    cur_start: u64,
    cur_inode_num: u32,
}

impl<TI> ReadDir<TI>
where TI: Iterator<Item = metadata::DirTable>
{
    fn new(table_iter: TI) -> Self
    {
        ReadDir { table_iter, cur_iter: None, cur_start: 0, cur_inode_num: 0 }
    }
}

impl<TI> Iterator for ReadDir<TI>
where TI: Iterator<Item = metadata::DirTable>
{
    type Item = DirEntry;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.cur_iter.is_none() {
                match self.table_iter.next() {
                    None => return None,
                    Some(t) => {
                        self.cur_start = t.start.into();
                        self.cur_inode_num = t.inode_number.into();
                        self.cur_iter = Some(t.entries.into_iter());
                    }
                }
            }
            match &mut self.cur_iter {
                Some(i) => {
                    match i.next() {
                        Some(e) => return Some(DirEntry::new(self.cur_start, self.cur_inode_num, e)),
                        None => {
                            self.cur_iter = None;
                            self.cur_start = 0;
                        }
                    }
                },
                None => panic!("cur_iter cannot be None!"),
            }
        }
    }
}