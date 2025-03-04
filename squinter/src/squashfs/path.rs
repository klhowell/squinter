// Note, quotes in this code are taken from the Open Group Base Specifications,
// Section 4.11 Pathname Resolution, found here:
// https://pubs.opengroup.org/onlinepubs/009696699/basedefs/xbd_chap04.html

use std::io::{self, ErrorKind, Read, Seek};
use std::path::{Component, Path, PathBuf};

use crate::squashfs::metadata::InodeExtendedInfo;

use super::squashfs::{DirEntry, SquashFS};

pub fn canonicalize<R,P,Q>(sqfs: &mut SquashFS<R>, path: P, cwd: Q) -> io::Result<PathBuf> 
where P: AsRef<Path>,
      Q: AsRef<Path>,
      R: Read + Seek,
{
    // "A null pathname shall not be successfully resolved"
    if path.as_ref().as_os_str().is_empty() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "Path is empty"));
    }

    // "If the pathname does not begin with a '/', the predecessor of the first filename of the pathname shall
    //  [be] the current working directory of the process..."
    let target_path = if path.as_ref().is_absolute() {
        path.as_ref().to_path_buf()
    } else {
        cwd.as_ref().join(path)
    };
    
    resolve_absolute_path(sqfs, target_path)
}

fn resolve_absolute_path<R,P>(sqfs: &mut SquashFS<R>, path: P) -> io::Result<PathBuf>
where P: AsRef<Path>,
      R: Read + Seek,
{
    assert!(path.as_ref().is_absolute());
    assert!(!path.as_ref().as_os_str().is_empty());

    // "A pathname the contains at least one non-'/' character and that ends with one or more trailing '/'
    // characters shall not be resolved successfully unless the last pathname component before the trailing '/'
    // characters names an existing directory..."
    let trailing_slash = path.as_ref().ends_with("/");
    
    // So that we can navigate to a parent component without having to re-resolve the entire path, as each component
    // is resolved, add it to a vector of components with their corresponding Inodes.
    let mut resolved_components: Vec<DirEntry> = Vec::new();
    
    // working_path is initially the entire absolute path. However, when symbolic links are traversed,
    // working_path may be replaced by a concatenation of the symbolic link target with the remaining
    // user-specified path. remaining_path is a cursor into working_path so that it does not need to
    // be re-allocated as each component is consumed into resolved_components.
    let mut working_path = path.as_ref().to_path_buf();
    let mut remaining_path = working_path.as_path();
    while let Some(comp) = remaining_path.components().next() {
        match comp {
            Component::RootDir => {
                assert!(resolved_components.is_empty());
                remaining_path = remaining_path.strip_prefix("/").map_err(|e| io::Error::new(ErrorKind::Other, e))?;
            },
            Component::CurDir => {
                remaining_path = remaining_path.strip_prefix(".").map_err(|e| io::Error::new(ErrorKind::Other, e))?;
            },
            Component::ParentDir => {
                // "as a special case, in the root directory, dot-dot may refer to the root directory itself."
                if !resolved_components.is_empty() {
                    resolved_components.pop();
                }
                remaining_path = remaining_path.strip_prefix("..").map_err(|e| io::Error::new(ErrorKind::Other, e))?;
            },
            Component::Normal(c) => {
                // Get the directory inode that should contain this component
                let parent_inode = if let Some(de) = resolved_components.last() {
                    sqfs.inode_from_entryref(de.inode_ref())?
                } else {
                    sqfs.root_inode()?
                };
                
                // Search the directory for a dirent with the correct name
                let dirent = sqfs.read_dir_inode(&parent_inode)?.find(|de| {
                    de.file_name().as_str() == c
                }).ok_or(io::Error::from(ErrorKind::NotFound))?;

                let inode = sqfs.inode_from_entryref(dirent.inode_ref())?;
                if inode.is_symlink() {
                    // TODO: If this is the last component of the requested path then we may not want to dereference it.
                    // See https://pubs.opengroup.org/onlinepubs/9699919799/basedefs/V1_chap04.html#tag_04_13

                    // If this dirent is a symbolic link then substitute its contents:
                    //   - Empty, return error
                    //   - relative path, insert the contents at the current position
                    //   - absolute path, restart resolution with the symbolic link contents as the first component
                    let target = match &inode.extended_info {
                        InodeExtendedInfo::BasicSymlink(i) => {
                            let target_path = i.target_path.to_str().map_err(
                                |e| io::Error::new(ErrorKind::InvalidData, e))?;
                            PathBuf::from(target_path)
                        }
                        // TODO: ExtSymlink
                        _ => return Err(io::Error::from(ErrorKind::Unsupported)),
                    };
                    if target.as_os_str().is_empty() {
                        return Err(io::Error::from(ErrorKind::InvalidData));
                    }
                    if target.is_absolute() {
                        resolved_components.clear();
                    }
                    remaining_path = remaining_path.strip_prefix(c).map_err(|e| io::Error::new(ErrorKind::Other, e))?;
                    working_path = target.join(remaining_path);
                    remaining_path = working_path.as_path();
                } else {
                    resolved_components.push(dirent);
                    remaining_path = remaining_path.strip_prefix(c).map_err(|e| io::Error::new(ErrorKind::Other, e))?;
                }
            },
            Component::Prefix(_) => {
                return Err(io::Error::from(ErrorKind::InvalidData));
            }
        }
    }

    let mut resolved = PathBuf::from("/");
    for de in resolved_components {
        resolved.push(de.file_name());
    }
    // TODO: What about the trailing slash???
    Ok(resolved)
}