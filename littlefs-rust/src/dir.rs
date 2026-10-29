use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;

use littlefs_rust_core::{LfsDir, LfsInfo};

use crate::error::{from_lfs_result, Error};
use crate::filesystem::Filesystem;
use crate::metadata::{DirEntry, FileType};
use crate::storage::Storage;

// The core retains this address in its open-handle list and updates it during
// directory commits. Rc keeps the allocation stable; UnsafeCell permits these
// mutations while access to the filesystem is serialized by its RefCell.
pub(crate) struct DirAllocation {
    pub(crate) dir: UnsafeCell<MaybeUninit<LfsDir>>,
}

impl DirAllocation {
    pub(crate) fn new() -> Self {
        Self {
            dir: UnsafeCell::new(MaybeUninit::zeroed()),
        }
    }
}

/// An open directory iterator.
///
/// Obtained from [`Filesystem::read_dir`]. Yields [`DirEntry`] items,
/// automatically skipping `.` and `..`. Closed on drop, or explicitly
/// via [`ReadDir::close`].
pub struct ReadDir<'a, S: Storage> {
    fs: &'a Filesystem<S>,
    alloc: Rc<DirAllocation>,
    closed: bool,
}

impl<'a, S: Storage> ReadDir<'a, S> {
    pub(crate) fn open(fs: &'a Filesystem<S>, path: &str) -> Result<Self, Error> {
        let alloc = Rc::new(DirAllocation::new());
        let path_bytes = null_terminate(path);
        {
            let mut inner = fs.inner.borrow_mut();
            let rc = littlefs_rust_core::lfs_dir_open(
                inner.lfs.as_mut_ptr(),
                alloc.dir.get().cast::<LfsDir>(),
                path_bytes.as_ptr(),
            );
            from_lfs_result(rc)?;
        }
        Ok(ReadDir {
            fs,
            alloc,
            closed: false,
        })
    }

    /// Close the directory handle. Consumes `self`.
    ///
    /// Dropping a [`ReadDir`] also closes it, but errors are silently ignored.
    pub fn close(mut self) -> Result<(), Error> {
        self.closed = true;
        let mut inner = self.fs.inner.borrow_mut();
        let rc = littlefs_rust_core::lfs_dir_close(
            inner.lfs.as_mut_ptr(),
            self.alloc.dir.get().cast::<LfsDir>(),
        );
        from_lfs_result(rc)
    }
}

impl<S: Storage> Iterator for ReadDir<'_, S> {
    type Item = Result<DirEntry, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let mut info = MaybeUninit::<LfsInfo>::zeroed();
            let rc = {
                let mut inner = self.fs.inner.borrow_mut();
                littlefs_rust_core::lfs_dir_read(
                    inner.lfs.as_mut_ptr(),
                    self.alloc.dir.get().cast::<LfsDir>(),
                    info.as_mut_ptr(),
                )
            };

            return match rc {
                0 => None,
                n if n < 0 => Some(Err(Error::from(n))),
                _ => {
                    let entry = dir_entry_from_info(unsafe { &*info.as_ptr() });
                    if entry.name == "." || entry.name == ".." {
                        continue;
                    }
                    Some(Ok(entry))
                }
            };
        }
    }
}

impl<S: Storage> Drop for ReadDir<'_, S> {
    fn drop(&mut self) {
        if !self.closed {
            if let Ok(mut inner) = self.fs.inner.try_borrow_mut() {
                let _ = littlefs_rust_core::lfs_dir_close(
                    inner.lfs.as_mut_ptr(),
                    self.alloc.dir.get().cast::<LfsDir>(),
                );
            }
        }
    }
}

pub(crate) fn dir_entry_from_info(info: &LfsInfo) -> DirEntry {
    let nul = info
        .name
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(info.name.len());
    let name = core::str::from_utf8(&info.name[..nul]).unwrap_or("").into();
    let file_type = if info.type_ == littlefs_rust_core::lfs_type::lfs_type::LFS_TYPE_DIR as u8 {
        FileType::Dir
    } else {
        FileType::File
    };
    DirEntry {
        name,
        file_type,
        size: info.size,
    }
}

fn null_terminate(s: &str) -> Vec<u8> {
    let mut v: Vec<u8> = s.bytes().collect();
    v.push(0);
    v
}
