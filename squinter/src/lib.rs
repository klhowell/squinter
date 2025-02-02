//! Squinter provides a read-only ***Squ***ashFS ***inter***face. The API focuses on performing
//! typical filesystem operations such as listing directory contents, reading files, etc.
//! 
//! ## Feature Flags
//! Squinter does not currently support any feature flags
//! 
//! ### no_std support
//! Squinter does not currently support no_std, but it doesn't have any deep dependencies on std,
//! either. no_std could likely be implemented with minimal effort.
//! 
//! ## Usage Example
//! ```rust
//! use std::io;
//! use squinter::squashfs::SquashFS;
//! fn print_file_from_squashfs() -> io::Result<()>{
//!     // Open the SquashFS
//!     let mut sqfs = SquashFS::open("rootfs.squashfs")?;
//! 
//!     // List the contents of a directory
//!     for d in sqfs.read_dir("/etc")? {
//!         println!("{}", d.file_name())
//!     }
//! 
//!     // Open a file to read its contents
//!     let mut file_reader = sqfs.open_file("/etc/group")?;
//! 
//!     // Copy the file contents to stdout
//!     let mut stdout = io::stdout().lock();
//!     io::copy(&mut file_reader, &mut stdout)?;
//!     Ok(())
//! }
//! ```

pub mod squashfs;
