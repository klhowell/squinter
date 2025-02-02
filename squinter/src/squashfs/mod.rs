mod squashfs;
mod filedata;
mod readermux;
mod superblock;

pub mod metadata;
pub use squashfs::*;
pub use metadata::Inode;