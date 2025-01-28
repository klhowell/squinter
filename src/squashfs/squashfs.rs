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
}

impl SquashFS<File> {
    pub fn open<P>(path: P) -> io::Result<Self>
    where P: AsRef<Path>
    {
        Self::new(File::open(path)?)
    }
}

impl<R: Read + Seek> SquashFS<R> {
    pub fn new(mut r: R) -> io::Result<Self>
    {
        r.seek(SeekFrom::Start(0))?;
        let sb = Superblock::read(&mut r)?;
        let md_reader = CachingMetadataReader::new(r, sb.compressor);
        Ok(SquashFS { md_reader, sb })
    }

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

    pub fn read_dir_entry(&mut self, dir_entry: &DirEntry) -> io::Result<ReadDir<std::vec::IntoIter<metadata::DirTable>>>
    {
        let inode = self.inode(dir_entry.inode_ref)?;
        let dir_tables = metadata::DirTable::read_for_inode(&mut self.md_reader, &self.sb, &inode)?;
        Ok(ReadDir::new(dir_tables.into_iter()))
    }

    pub fn read_dir_inode(&mut self, inode: &metadata::Inode) -> io::Result<ReadDir<std::vec::IntoIter<metadata::DirTable>>>
    {
        let dir_tables = metadata::DirTable::read_for_inode(&mut self.md_reader, &self.sb, inode)?;
        Ok(ReadDir::new(dir_tables.into_iter()))
    }

    pub fn open_file_inode(&mut self, inode: &metadata::Inode, file_reader: R) -> io::Result<FileDataReader<R>> {
        Ok(FileDataReader::from_inode(file_reader, &mut self.md_reader, &self.sb, inode)?.unwrap())
    }

    pub fn root_inode(&mut self) -> io::Result<metadata::Inode> {
        metadata::Inode::read_at_ref(&mut self.md_reader, &self.sb, self.sb.root_inode)
    }

    pub fn inode(&mut self, inode_ref: metadata::EntryReference) -> io::Result<metadata::Inode> {
        metadata::Inode::read_at_ref(&mut self.md_reader, &self.sb, inode_ref)
    }
}

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