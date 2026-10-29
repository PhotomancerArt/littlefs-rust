use alloc::rc::Rc;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::mem::MaybeUninit;

use littlefs_rust_core::{LfsFile, LfsFileConfig};

use crate::error::{from_lfs_result, from_lfs_size, Error};
use crate::filesystem::Filesystem;
use crate::metadata::{OpenFlags, SeekFrom};
use crate::storage::Storage;

pub(crate) struct FileAllocation {
    pub(crate) file: UnsafeCell<MaybeUninit<LfsFile>>,
    _cache: Vec<u8>,
    pub(crate) file_config: LfsFileConfig,
}

impl FileAllocation {
    pub(crate) fn new(cache_size: u32) -> Self {
        let mut cache = vec![0u8; cache_size as usize];
        let file_config = LfsFileConfig {
            buffer: cache.as_mut_ptr() as *mut c_void,
            attrs: core::ptr::null_mut(),
            attr_count: 0,
        };
        Self {
            file: UnsafeCell::new(MaybeUninit::zeroed()),
            _cache: cache,
            file_config,
        }
    }
}

/// An open file handle.
///
/// Obtained from [`Filesystem::open`]. Automatically closed on drop; call
/// [`File::close`] explicitly to check for errors.
pub struct File<'a, S: Storage> {
    fs: &'a Filesystem<S>,
    // Core retains this handle in its open-file list. Rc keeps its address
    // stable without exclusive Box reborrows; UnsafeCell permits mutation
    // through the shared File API while the filesystem RefCell serializes it.
    alloc: Rc<FileAllocation>,
    closed: bool,
}

impl<'a, S: Storage> File<'a, S> {
    pub(crate) fn open(fs: &'a Filesystem<S>, path: &str, flags: OpenFlags) -> Result<Self, Error> {
        let alloc = Rc::new(FileAllocation::new(fs.cache_size()));
        let path_bytes = null_terminate(path);
        {
            let mut inner = fs.inner.borrow_mut();
            let rc = littlefs_rust_core::lfs_file_opencfg(
                inner.lfs.as_mut_ptr(),
                alloc.file.get().cast::<LfsFile>(),
                path_bytes.as_ptr(),
                flags.bits() as i32,
                &alloc.file_config as *const LfsFileConfig,
            );
            from_lfs_result(rc)?;
        }
        Ok(File {
            fs,
            alloc,
            closed: false,
        })
    }

    /// Read up to `buf.len()` bytes from the current position.
    /// Returns the number of bytes actually read.
    pub fn read(&self, buf: &mut [u8]) -> Result<u32, Error> {
        let mut inner = self.fs.inner.borrow_mut();
        let rc = littlefs_rust_core::lfs_file_read(
            inner.lfs.as_mut_ptr(),
            self.alloc.file.get().cast::<LfsFile>(),
            buf.as_mut_ptr() as *mut c_void,
            buf.len() as u32,
        );
        drop(inner);
        from_lfs_size(rc)
    }

    /// Write `data` at the current position. Returns the number of bytes written.
    pub fn write(&self, data: &[u8]) -> Result<u32, Error> {
        let mut inner = self.fs.inner.borrow_mut();
        let rc = littlefs_rust_core::lfs_file_write(
            inner.lfs.as_mut_ptr(),
            self.alloc.file.get().cast::<LfsFile>(),
            data.as_ptr() as *const c_void,
            data.len() as u32,
        );
        drop(inner);
        from_lfs_size(rc)
    }

    /// Seek to a position. Returns the new absolute offset.
    pub fn seek(&self, pos: SeekFrom) -> Result<u32, Error> {
        let (off, whence) = match pos {
            SeekFrom::Start(n) => (
                n as i32,
                littlefs_rust_core::lfs_type::lfs_whence_flags::LFS_SEEK_SET,
            ),
            SeekFrom::Current(n) => (
                n,
                littlefs_rust_core::lfs_type::lfs_whence_flags::LFS_SEEK_CUR,
            ),
            SeekFrom::End(n) => (
                n,
                littlefs_rust_core::lfs_type::lfs_whence_flags::LFS_SEEK_END,
            ),
        };
        let mut inner = self.fs.inner.borrow_mut();
        let rc = littlefs_rust_core::lfs_file_seek(
            inner.lfs.as_mut_ptr(),
            self.alloc.file.get().cast::<LfsFile>(),
            off,
            whence,
        );
        drop(inner);
        from_lfs_size(rc)
    }

    /// Return the current read/write position.
    pub fn tell(&self) -> u32 {
        let mut inner = self.fs.inner.borrow_mut();
        let rc = littlefs_rust_core::lfs_file_tell(
            inner.lfs.as_mut_ptr(),
            self.alloc.file.get().cast::<LfsFile>(),
        );
        drop(inner);
        rc as u32
    }

    /// Return the file size in bytes.
    pub fn size(&self) -> u32 {
        let mut inner = self.fs.inner.borrow_mut();
        let rc = littlefs_rust_core::lfs_file_size(
            inner.lfs.as_mut_ptr(),
            self.alloc.file.get().cast::<LfsFile>(),
        );
        drop(inner);
        rc as u32
    }

    /// Flush cached writes to storage.
    pub fn sync(&self) -> Result<(), Error> {
        let mut inner = self.fs.inner.borrow_mut();
        let rc = littlefs_rust_core::lfs_file_sync(
            inner.lfs.as_mut_ptr(),
            self.alloc.file.get().cast::<LfsFile>(),
        );
        drop(inner);
        from_lfs_result(rc)
    }

    /// Truncate or extend the file to `size` bytes.
    pub fn truncate(&self, size: u32) -> Result<(), Error> {
        let mut inner = self.fs.inner.borrow_mut();
        let rc = littlefs_rust_core::lfs_file_truncate(
            inner.lfs.as_mut_ptr(),
            self.alloc.file.get().cast::<LfsFile>(),
            size,
        );
        drop(inner);
        from_lfs_result(rc)
    }

    /// Close the file, flushing any pending writes. Consumes `self`.
    ///
    /// Dropping a [`File`] also closes it, but errors are silently ignored.
    pub fn close(mut self) -> Result<(), Error> {
        self.closed = true;
        let mut inner = self.fs.inner.borrow_mut();
        let rc = littlefs_rust_core::lfs_file_close(
            inner.lfs.as_mut_ptr(),
            self.alloc.file.get().cast::<LfsFile>(),
        );
        from_lfs_result(rc)
    }
}

impl<S: Storage> Drop for File<'_, S> {
    fn drop(&mut self) {
        if !self.closed {
            if let Ok(mut inner) = self.fs.inner.try_borrow_mut() {
                let _ = littlefs_rust_core::lfs_file_close(
                    inner.lfs.as_mut_ptr(),
                    self.alloc.file.get().cast::<LfsFile>(),
                );
            }
        }
    }
}

fn null_terminate(s: &str) -> Vec<u8> {
    let mut v: Vec<u8> = s.bytes().collect();
    v.push(0);
    v
}
