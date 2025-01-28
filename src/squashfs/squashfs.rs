// See https://dr-emann.github.io/squashfs/squashfs.html
// for details on the SquashFS binary format

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Component, Path};

use super::filedata::FileDataReader;
use super::metadata::{self, CachingMetadataReader};
use super::superblock::Superblock;

#[derive(Debug)]
pub struct SquashFS<R> {
    md_reader: CachingMetadataReader<R>,
    sb: Superblock,
    pub id_table: metadata::IdLookupTable,
}

impl SquashFS<File> {
    /// Open a filepath as a SquashFS
    pub fn open<P>(path: P) -> io::Result<Self>
    where P: AsRef<Path>
    {
        Self::new(File::open(path)?)
    }
}

impl<R: Read + Seek> SquashFS<R> {
    /// Create a new SquashFS instance from the provided Reader
    pub fn new(mut r: R) -> io::Result<Self>
    {
        r.seek(SeekFrom::Start(0))?;
        let sb = Superblock::read(&mut r)?;
        let id_table = metadata::IdLookupTable::read(&mut r, &sb)?;
        let md_reader = CachingMetadataReader::new(r, sb.compressor);
        Ok(SquashFS { md_reader, sb, id_table })
    }

    /// Retrieve an iterator that walks the dirents within a directory specified by the given
    /// path.
    /// path must refer to an existing directory or this function returns an error.
    pub fn read_dir<P>(&mut self, path: P) -> io::Result<ReadDir<std::vec::IntoIter<metadata::DirTable>>>
    where P: AsRef<Path>
    {
        let mut inode = self.root_inode()?;
        for comp in path.as_ref().components() {
            inode = match comp {
                Component::RootDir => self.root_inode()?,
                Component::Normal(n) => {
                    self.read_dir_inode(&inode)?.find(|e| n == e.file_name().as_str())
                        .map(|e| self.inode(e.inode_ref()).ok())
                        .flatten()
                        .ok_or(io::Error::from(io::ErrorKind::NotFound))?
                },
                _ => { return Err(io::Error::new(io::ErrorKind::InvalidData, "Error parsing filepath"))}
            };
        }
        let dir_tables = metadata::DirTable::read_for_inode(&mut self.md_reader, &self.sb, &inode)?;
        Ok(ReadDir::new(dir_tables.into_iter()))
    }

    /// Retrieve an iterator that walks the dirents within a directory specified by the given
    /// DirEntry.
    pub fn read_dir_entry(&mut self, dir_entry: &DirEntry) -> io::Result<ReadDir<std::vec::IntoIter<metadata::DirTable>>>
    {
        let inode = self.inode(dir_entry.inode_ref)?;
        let dir_tables = metadata::DirTable::read_for_inode(&mut self.md_reader, &self.sb, &inode)?;
        Ok(ReadDir::new(dir_tables.into_iter()))
    }

    /// Retrieve an iterator that walks the dirents within a directory specified by the given
    /// Inode.
    pub fn read_dir_inode(&mut self, inode: &metadata::Inode) -> io::Result<ReadDir<std::vec::IntoIter<metadata::DirTable>>>
    {
        let dir_tables = metadata::DirTable::read_for_inode(&mut self.md_reader, &self.sb, inode)?;
        Ok(ReadDir::new(dir_tables.into_iter()))
    }

    /// Create an IO reader for the contents of the files specified by the given Inode
    pub fn open_file_inode(&mut self, inode: &metadata::Inode, file_reader: R) -> io::Result<FileDataReader<R>> {
        Ok(FileDataReader::from_inode(file_reader, &mut self.md_reader, &self.sb, inode)?.unwrap())
    }

    /// Retrieve the root Inode of the SquashFS
    pub fn root_inode(&mut self) -> io::Result<metadata::Inode> {
        metadata::Inode::read_at_ref(&mut self.md_reader, &self.sb, self.sb.root_inode)
    }

    /// Retrieve the Inode specified by SquashFS metadata Entry Reference
    pub fn inode(&mut self, inode_ref: metadata::EntryReference) -> io::Result<metadata::Inode> {
        metadata::Inode::read_at_ref(&mut self.md_reader, &self.sb, inode_ref)
    }
}

// A DirEntry, like in std::fs, represents a named inode-reference within a directory. For example, a filename
// together with a reference to the file's inode.
pub struct DirEntry {
    inner: metadata::DirEntry,
    inode_ref: metadata::EntryReference,
}

impl DirEntry {
    pub fn new(dt_start: u64, inner: metadata::DirEntry) -> Self {
        let inode_ref = metadata::EntryReference::new(dt_start, inner.offset);
        Self { inner, inode_ref }
    }

    pub fn file_name(&self) -> String {
        self.inner.name.to_str().unwrap().to_string()
    }

    pub fn inode_ref(&self) -> metadata::EntryReference {
        self.inode_ref
    }
}

// An iterator over the individual DirEntries in a list of DirEntry runs. The SquashFS filesystem
// potentially splits the DirEntries for a directory table across multiple runs, so this object must
// combine walking of both runs and entries into the next() Iterator function.
pub struct ReadDir<TI> {
    table_iter: TI,
    cur_iter: Option<std::vec::IntoIter<metadata::DirEntry>>,
    cur_start: u64,
}

impl<TI> ReadDir<TI>
where TI: Iterator<Item = metadata::DirTable>
{
    fn new(table_iter: TI) -> Self
    {
        ReadDir { table_iter, cur_iter: None, cur_start: 0 }
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
                        self.cur_iter = Some(t.entries.into_iter());
                    }
                }
            }
            match &mut self.cur_iter {
                Some(i) => {
                    match i.next() {
                        Some(e) => return Some(DirEntry::new(self.cur_start, e)),
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