/// Dump contents of a SquashFS starting at a specified path
use std::env;
use std::io::{Read, Seek};

use anyhow;
use squinter::squashfs::{self, SquashFS};

fn main() -> anyhow::Result<()> {
    let sqfs_path = env::args().nth(1).unwrap();
    let p = env::args().nth(2).unwrap();

    let mut sqfs = SquashFS::open(&sqfs_path)?;
    let i = sqfs.inode_from_path(p)?;
    read_tree_sqfs(&mut sqfs, i, true)?;
    //read_and_descend_sqfs(&mut sqfs, &i, true)?;
    Ok(())
}

fn read_tree_sqfs<R: Read + Seek>(sqfs: &mut squashfs::SquashFS<R>, top_node: squashfs::metadata::Inode, content: bool) 
    -> anyhow::Result<()> {
    let mut nodes = Vec::new();
    nodes.push(top_node);
    
    while let Some(node) = nodes.pop() {
        assert!(node.is_dir());
        let dir = sqfs.read_dir_inode(&node)?;
        for de in dir {
            let n = sqfs.inode_from_entryref(de.inode_ref())?;
            if content && n.is_file() {
                let mut r = sqfs.open_file_inode(&n)?;
                std::io::copy(&mut r, &mut std::io::sink())?;
            } else if n.is_dir() {
                nodes.push(n);
            }
        }
    }
    Ok(())
}

fn read_and_descend_sqfs<R: Read + Seek>(sqfs: &mut squashfs::SquashFS<R>, sq_inode: &squashfs::metadata::Inode, content: bool)
    -> anyhow::Result<u32>
{
    assert!(sq_inode.is_dir());

    let sqfs_dir = sqfs.read_dir_inode(sq_inode)?;

    let mut total = 0;
    for de in sqfs_dir {
        let sq_inode = sqfs.inode_from_entryref(de.inode_ref())?;
        if content && sq_inode.is_file() {
            let mut sq_reader = sqfs.open_file_inode(&sq_inode)?;
            std::io::copy(&mut sq_reader, &mut std::io::sink())?;
        }
        // If the inode represents a directory, recurse to the directory contents
        if sq_inode.is_dir() {
            total += read_and_descend_sqfs(sqfs, &sq_inode, content)?;
        }
        total += 1;
    }
    Ok(total)
}
