use std::fs::File;
use std::io::{self, Read};
use std::io::{Seek, SeekFrom};
use std::path::{PathBuf, Path};

use squashfs_tools::squashfs::metadata::{self, InodeType, EntryReference};
use squashfs_tools::squashfs::superblock::Superblock;

#[test]
fn test_parse_all() -> io::Result<()> {
    let mut f = File::open("test_data/1.squashfs")?;

    let sb = Superblock::read(&mut f)?;
    println!("Superblock = {:?}", sb);

    let id_table = metadata::IdLookupTable::read(&mut f, &sb)?;
    println!("ID Table = {:?}", id_table);

    let export_table = metadata::ExportLookupTable::read(&mut f, &sb)?;
    match export_table {
        Some(t) => println!("Export Table = ({} Entries)", t.lu_table.entries.len()),
        None => println!("No export table"),
    }

    let xattr_table = metadata::ExtendedAttributeLookupTable::read(&mut f, &sb)?;
    match xattr_table {
        Some(t) => println!("XATTR Table = {:?}", t),
        None => println!("No xattr table"),
    }

    let mut buf: [u8; 8192] = [0; 8192];
    /*
    f.seek(SeekFrom::Start(id_table.block_offsets[0]))?;
    f.read(&mut buf[0..22])?;
    println!("Data = {:02x?}", &buf[0..22]);

    f.seek(SeekFrom::Start(id_table.block_offsets[0]))?;
    let size = metadata::read_metadata_block(&mut f, &sb.compressor, &mut buf)?;
    println!("ID Table size = {}", size);
    */
    f.seek(SeekFrom::Start(sb.inode_table))?;
    let size = metadata::read_metadata_block(&mut f, &sb.compressor, &mut buf)?;
    println!("First Inode Block size = {}", size.1);
    let mut buf_reader = &buf[..];
    for _ in 0..16 {
        let inode = metadata::Inode::read(&mut buf_reader, sb.block_size)?;
        println!("Inode = {:?}", inode);
    }

    f.seek(SeekFrom::Start(sb.dir_table))?;
    let size = metadata::read_metadata_block(&mut f, &sb.compressor, &mut buf)?;
    println!("First Dir Block size = {}", size.1);
    let mut buf_reader = &buf[..];
    let dir_table = metadata::DirTable::load(&mut buf_reader)?;
    println!("Dir Table = {:?}", dir_table);

    Ok(())
}

#[test]
fn test_list_all() -> io::Result<()> {
    let mut f = File::open("test_data/1.squashfs")?;
    let sb = Superblock::read(&mut f)?;

    //let mut mdr = metadata::MetadataReader::new(f, sb.compressor);
    let mut mdr = metadata::CachingMetadataReader::new(f, sb.compressor);

    let inode = metadata::Inode::read_at_ref(&mut mdr, &sb, sb.root_inode)?;
    println!("Root Inode : {:#?}", inode);

    let path = PathBuf::from("squashfs-root");
    let dir_table = metadata::DirTable::read_for_inode(&mut mdr, &sb, &inode)?;
    for dt in &dir_table {
        for de in &dt.entries {
            print_and_descend(&mut mdr, &sb, &dt, &de, &path)?
        }
    }
    //println!("Root DirTable : {:#?}", dir_table);

    Ok(())
}

fn print_and_descend<R>(mdr: &mut R, sb: &Superblock, dir_table: &metadata::DirTable, dir_entry: &metadata::DirEntry, base_path: &Path) -> io::Result<()>
where R: Read + Seek
{
    println!("{}", base_path.join(dir_entry.name.to_str().unwrap()).to_str().unwrap());
    if let InodeType::BasicDir = dir_entry.inode_type {
        let path = base_path.join(dir_entry.name.to_str().unwrap());
        let inode_ref = EntryReference::new(dir_table.start.into(), dir_entry.offset);
        let inode = metadata::Inode::read_at_ref(mdr, sb, inode_ref)?;
        //println!("Inode: {:#?}", inode);
        let dir_table = metadata::DirTable::read_for_inode(mdr, sb, &inode)?;
        for dt in &dir_table {
            for de in &dt.entries {
                print_and_descend(mdr, sb, &dt, &de, &path)?
            }
        }
    }
    Ok(())

}
