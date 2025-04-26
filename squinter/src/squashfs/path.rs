// Note, quotes in this code are taken from the Open Group Base Specifications,
// Section 4.11 Pathname Resolution, found here:
// https://pubs.opengroup.org/onlinepubs/009696699/basedefs/xbd_chap04.html

use std::io::{self, ErrorKind, Read, Seek};
use std::path::{Component, Path, PathBuf};

use crate::squashfs::metadata::InodeExtendedInfo;

use super::squashfs::{DirEntry, SquashFS};

/// Return the canonical, absolute form of the provided path with all intermediate components
/// normalized and all symbolic links resolved.
///  - If the path is relative, the CWD is prepended
///  - If the path contains symbolic links, they are replaced with their targets
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

/// Walk the components of the path and resolve all symbolic links according to the open group
/// rules.
fn resolve_absolute_path<R,P>(sqfs: &mut SquashFS<R>, path: P) -> io::Result<PathBuf>
where P: AsRef<Path>,
      R: Read + Seek,
{
    assert!(path.as_ref().is_absolute());
    assert!(!path.as_ref().as_os_str().is_empty());

    // "A pathname the contains at least one non-'/' character and that ends with one or more trailing '/'
    // characters shall not be resolved successfully unless the last pathname component before the trailing '/'
    // characters names an existing directory..."
    let trailing_slash = path.as_ref().to_str().unwrap().ends_with("/");

    // So that we can navigate to a parent component without having to re-resolve the entire path, as each component
    // is resolved, add it to a vector of components with their corresponding Inodes.
    let mut resolved_components: Vec<DirEntry> = Vec::new();

    // working_path is initially the entire absolute path. However, when symbolic links are traversed,
    // working_path may be replaced by a concatenation of the symbolic link target with the remaining
    // user-specified path. When this happens, iteration resumes from the beginning of the remaining
    // path components.
    let mut working_path = path.as_ref().to_path_buf();
    let mut path_components = working_path.components();
    while let Some(comp) = path_components.next() {
        let last_comp = path_components.as_path().as_os_str().is_empty();
        match comp {
            Component::RootDir => {
                assert!(resolved_components.is_empty());
            },
            Component::CurDir => {
            },
            Component::ParentDir => {
                // "as a special case, in the root directory, dot-dot may refer to the root directory itself."
                if !resolved_components.is_empty() {
                    resolved_components.pop();
                }
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
                    // "If all of the following are true, then pathname resolution is complete:
                    //  1. This is the last pathname component of the pathname.
                    //  2. The pathname has no trailing slash
                    //  3. The function is required to act on the symbolic link itself..."
                    if last_comp && !trailing_slash {
                        // The symbolic link itself is the final path component
                        resolved_components.push(dirent);
                    } else {
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
                        working_path = target.join(path_components.as_path());
                        path_components = working_path.components();
                    }
                } else {
                    resolved_components.push(dirent);
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