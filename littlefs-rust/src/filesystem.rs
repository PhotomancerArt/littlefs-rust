use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::ffi::c_void;
use core::mem::{ManuallyDrop, MaybeUninit};

use littlefs_rust_core::{Lfs, LfsConfig, LfsInfo, LFS_ERR_IO};

use crate::config::Config;
use crate::dir::{dir_entry_from_info, ReadDir};
use crate::error::{from_lfs_result, from_lfs_size, Error};
use crate::file::File;
use crate::metadata::{DirEntry, Metadata, OpenFlags};
use crate::storage::Storage;

pub(crate) struct FsInner<S: Storage> {
    pub(crate) lfs: MaybeUninit<Lfs>,
    // Core retains pointers to the configuration and storage across wrapper
    // borrows. Keep their pointees separate from mutable FsInner reborrows.
    pub(crate) config: Box<LfsConfig>,
    pub(crate) storage: Box<S>,
    _read_buf: Vec<u8>,
    _prog_buf: Vec<u8>,
    _lookahead_buf: Vec<u8>,
    pub(crate) mounted: bool,
}

/// A mounted LittleFS filesystem.
///
/// All methods take `&self` via interior mutability, so multiple [`File`] and
/// [`ReadDir`] handles can coexist. The internal state is heap-allocated and
/// pinned so that core pointers remain stable across moves.
///
/// Use [`Filesystem::format`] to initialize storage, then [`Filesystem::mount`]
/// to obtain a `Filesystem`. Call [`Filesystem::unmount`] to cleanly unmount
/// and recover the storage, or let [`Drop`] handle it automatically.
///
/// `Filesystem` is `!Send` and `!Sync` (due to interior `RefCell`). This is
/// appropriate for single-threaded embedded use. If you need cross-thread
/// access, wrap it in a `Mutex`.
pub struct Filesystem<S: Storage> {
    pub(crate) inner: RefCell<Box<FsInner<S>>>,
}

// ── Trampolines ─────────────────────────────────────────────────────────────

unsafe extern "C" fn trampoline_read<S: Storage>(
    cfg: *const LfsConfig,
    block: u32,
    off: u32,
    buffer: *mut u8,
    size: u32,
) -> i32 {
    let storage = &mut *((*cfg).context as *mut S);
    let buf = core::slice::from_raw_parts_mut(buffer, size as usize);
    match storage.read(block, off, buf) {
        Ok(()) => 0,
        Err(_) => LFS_ERR_IO,
    }
}

unsafe extern "C" fn trampoline_prog<S: Storage>(
    cfg: *const LfsConfig,
    block: u32,
    off: u32,
    buffer: *const u8,
    size: u32,
) -> i32 {
    let storage = &mut *((*cfg).context as *mut S);
    let buf = core::slice::from_raw_parts(buffer, size as usize);
    match storage.write(block, off, buf) {
        Ok(()) => 0,
        Err(_) => LFS_ERR_IO,
    }
}

unsafe extern "C" fn trampoline_erase<S: Storage>(cfg: *const LfsConfig, block: u32) -> i32 {
    let storage = &mut *((*cfg).context as *mut S);
    match storage.erase(block) {
        Ok(()) => 0,
        Err(_) => LFS_ERR_IO,
    }
}

unsafe extern "C" fn trampoline_sync<S: Storage>(cfg: *const LfsConfig) -> i32 {
    let storage = &mut *((*cfg).context as *mut S);
    match storage.sync() {
        Ok(()) => 0,
        Err(_) => LFS_ERR_IO,
    }
}

// ── FsInner construction ────────────────────────────────────────────────────

fn build_inner<S: Storage>(storage: S, config: &Config) -> FsInner<S> {
    let cache_size = config.resolve_cache_size() as usize;
    let lookahead_size = config.resolve_lookahead_size() as usize;

    let mut read_buf = vec![0u8; cache_size];
    let mut prog_buf = vec![0u8; cache_size];
    let mut lookahead_buf = vec![0u8; lookahead_size];

    let lfs_config = LfsConfig {
        context: core::ptr::null_mut(),
        read: Some(trampoline_read::<S>),
        prog: Some(trampoline_prog::<S>),
        erase: Some(trampoline_erase::<S>),
        sync: Some(trampoline_sync::<S>),
        read_size: config.read_size,
        prog_size: config.prog_size,
        block_size: config.block_size,
        block_count: config.block_count,
        block_cycles: config.block_cycles,
        cache_size: config.resolve_cache_size(),
        lookahead_size: config.resolve_lookahead_size(),
        compact_thresh: u32::MAX,
        read_buffer: read_buf.as_mut_ptr() as *mut c_void,
        prog_buffer: prog_buf.as_mut_ptr() as *mut c_void,
        lookahead_buffer: lookahead_buf.as_mut_ptr() as *mut c_void,
        name_max: config.name_max,
        file_max: config.file_max,
        attr_max: config.attr_max,
        metadata_max: 0,
        inline_max: 0,
    };

    FsInner {
        lfs: MaybeUninit::zeroed(),
        config: Box::new(lfs_config),
        storage: Box::new(storage),
        _read_buf: read_buf,
        _prog_buf: prog_buf,
        _lookahead_buf: lookahead_buf,
        mounted: false,
    }
}

/// Wire `config.context` to point at `inner.storage`. Must be called after
/// `inner` is at its final address (i.e., inside the `RefCell`).
fn wire_context<S: Storage>(inner: &mut FsInner<S>) {
    inner.config.context = &mut *inner.storage as *mut S as *mut c_void;
    inner.config.read_buffer = inner._read_buf.as_mut_ptr() as *mut c_void;
    inner.config.prog_buffer = inner._prog_buf.as_mut_ptr() as *mut c_void;
    inner.config.lookahead_buffer = inner._lookahead_buf.as_mut_ptr() as *mut c_void;
}

// ── Filesystem ──────────────────────────────────────────────────────────────

impl<S: Storage> Filesystem<S> {
    /// Format `storage` with a fresh LittleFS filesystem.
    ///
    /// This erases any existing data. The storage can be mounted afterwards
    /// with [`Filesystem::mount`].
    pub fn format(storage: &mut S, config: &Config) -> Result<(), Error> {
        let mut inner = build_inner_borrowed(storage, config);
        wire_context_borrowed(&mut inner);
        let rc = littlefs_rust_core::lfs_format(
            inner.lfs.as_mut_ptr(),
            &inner.config as *const LfsConfig,
        );
        from_lfs_result(rc)
    }

    /// Mount an existing filesystem. Takes ownership of the storage.
    ///
    /// On failure the storage is returned alongside the error so the caller
    /// can retry (e.g. format + mount).
    pub fn mount(storage: S, config: Config) -> Result<Self, (Error, S)> {
        let mut inner = Box::new(build_inner(storage, &config));
        wire_context(&mut inner);
        let rc = littlefs_rust_core::lfs_mount(
            inner.lfs.as_mut_ptr(),
            &*inner.config as *const LfsConfig,
        );
        if rc != 0 {
            return Err((Error::from(rc), *inner.storage));
        }
        inner.mounted = true;
        Ok(Filesystem {
            inner: RefCell::new(inner),
        })
    }

    /// Unmount and return the underlying storage.
    ///
    /// Prefer this over dropping when you need to check for errors or reuse
    /// the storage.
    pub fn unmount(self) -> Result<S, Error> {
        let this = ManuallyDrop::new(self);
        let mut inner = this.inner.borrow_mut();
        let rc = if inner.mounted {
            inner.mounted = false;
            littlefs_rust_core::lfs_unmount(inner.lfs.as_mut_ptr())
        } else {
            0
        };
        drop(inner);
        // Safety: we prevented Drop from running via ManuallyDrop, and we've
        // already unmounted. Take ownership of the RefCell's contents.
        let fs_inner = unsafe { core::ptr::read(&this.inner) }.into_inner();
        from_lfs_result(rc)?;
        Ok(*fs_inner.storage)
    }

    pub(crate) fn cache_size(&self) -> u32 {
        self.inner.borrow().config.cache_size
    }

    // ── File access ─────────────────────────────────────────────────────

    /// Open a file with the given [`OpenFlags`].
    ///
    /// Common combinations: `READ`, `WRITE | CREATE | TRUNC`,
    /// `WRITE | CREATE | APPEND`.
    pub fn open(&self, path: &str, flags: OpenFlags) -> Result<File<'_, S>, Error> {
        File::open(self, path, flags)
    }

    // ── Convenience file I/O ────────────────────────────────────────────

    /// Read an entire file into a `Vec<u8>`.
    pub fn read_to_vec(&self, path: &str) -> Result<Vec<u8>, Error> {
        let file = self.open(path, OpenFlags::READ)?;
        let size = file.size() as usize;
        let mut buf = vec![0u8; size];
        if size > 0 {
            let n = file.read(&mut buf)?;
            buf.truncate(n as usize);
        }
        Ok(buf)
    }

    /// Write `data` to a file, creating or truncating it.
    pub fn write_file(&self, path: &str, data: &[u8]) -> Result<(), Error> {
        let file = self.open(
            path,
            OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNC,
        )?;
        let mut offset = 0;
        while offset < data.len() {
            let n = file.write(&data[offset..])? as usize;
            offset += n;
        }
        Ok(())
    }

    // ── Path operations ─────────────────────────────────────────────────

    /// Create a directory. Fails if it already exists.
    pub fn mkdir(&self, path: &str) -> Result<(), Error> {
        let path_bytes = null_terminate(path);
        let mut inner = self.inner.borrow_mut();
        let rc = littlefs_rust_core::lfs_mkdir(inner.lfs.as_mut_ptr(), path_bytes.as_ptr());
        from_lfs_result(rc)
    }

    /// Remove a file or empty directory.
    pub fn remove(&self, path: &str) -> Result<(), Error> {
        let path_bytes = null_terminate(path);
        let mut inner = self.inner.borrow_mut();
        let rc = littlefs_rust_core::lfs_remove(inner.lfs.as_mut_ptr(), path_bytes.as_ptr());
        from_lfs_result(rc)
    }

    /// Rename or move a file or directory.
    pub fn rename(&self, from: &str, to: &str) -> Result<(), Error> {
        let from_bytes = null_terminate(from);
        let to_bytes = null_terminate(to);
        let mut inner = self.inner.borrow_mut();
        let rc = littlefs_rust_core::lfs_rename(
            inner.lfs.as_mut_ptr(),
            from_bytes.as_ptr(),
            to_bytes.as_ptr(),
        );
        from_lfs_result(rc)
    }

    /// Get metadata for a file or directory.
    pub fn stat(&self, path: &str) -> Result<Metadata, Error> {
        let path_bytes = null_terminate(path);
        let mut info = MaybeUninit::<LfsInfo>::zeroed();
        {
            let mut inner = self.inner.borrow_mut();
            let rc = littlefs_rust_core::lfs_stat(
                inner.lfs.as_mut_ptr(),
                path_bytes.as_ptr(),
                info.as_mut_ptr(),
            );
            from_lfs_result(rc)?;
        }
        let entry = dir_entry_from_info(unsafe { &*info.as_ptr() });
        Ok(Metadata {
            name: entry.name,
            file_type: entry.file_type,
            size: entry.size,
        })
    }

    /// Returns `true` if `path` exists.
    pub fn exists(&self, path: &str) -> bool {
        self.stat(path).is_ok()
    }

    // ── Directory listing ───────────────────────────────────────────────

    /// Open a directory for iteration. The returned [`ReadDir`] is an
    /// [`Iterator`] that skips `.` and `..` entries.
    pub fn read_dir(&self, path: &str) -> Result<ReadDir<'_, S>, Error> {
        ReadDir::open(self, path)
    }

    /// Collect all entries in a directory into a `Vec`.
    pub fn list_dir(&self, path: &str) -> Result<Vec<DirEntry>, Error> {
        let dir = self.read_dir(path)?;
        dir.collect()
    }

    // ── FS-level ────────────────────────────────────────────────────────

    /// Return the number of allocated blocks.
    pub fn fs_size(&self) -> Result<u32, Error> {
        let mut inner = self.inner.borrow_mut();
        let rc = littlefs_rust_core::lfs_fs_size(inner.lfs.as_mut_ptr());
        from_lfs_size(rc)
    }

    /// Run garbage collection to reclaim unused blocks.
    pub fn gc(&self) -> Result<(), Error> {
        let mut inner = self.inner.borrow_mut();
        let rc = littlefs_rust_core::lfs_fs_gc(inner.lfs.as_mut_ptr());
        from_lfs_result(rc)
    }
}

impl<S: Storage> Drop for Filesystem<S> {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.inner.try_borrow_mut() {
            if inner.mounted {
                let _ = littlefs_rust_core::lfs_unmount(inner.lfs.as_mut_ptr());
                inner.mounted = false;
            }
        }
    }
}

// ── format helper (borrows storage instead of taking ownership) ─────────────

struct BorrowedFsInner<'a, S: Storage> {
    lfs: MaybeUninit<Lfs>,
    config: LfsConfig,
    storage: &'a mut S,
    _read_buf: Vec<u8>,
    _prog_buf: Vec<u8>,
    _lookahead_buf: Vec<u8>,
}

fn build_inner_borrowed<'a, S: Storage>(
    storage: &'a mut S,
    config: &Config,
) -> BorrowedFsInner<'a, S> {
    let cache_size = config.resolve_cache_size() as usize;
    let lookahead_size = config.resolve_lookahead_size() as usize;

    let mut read_buf = vec![0u8; cache_size];
    let mut prog_buf = vec![0u8; cache_size];
    let mut lookahead_buf = vec![0u8; lookahead_size];

    let lfs_config = LfsConfig {
        context: core::ptr::null_mut(),
        read: Some(trampoline_read::<S>),
        prog: Some(trampoline_prog::<S>),
        erase: Some(trampoline_erase::<S>),
        sync: Some(trampoline_sync::<S>),
        read_size: config.read_size,
        prog_size: config.prog_size,
        block_size: config.block_size,
        block_count: config.block_count,
        block_cycles: config.block_cycles,
        cache_size: config.resolve_cache_size(),
        lookahead_size: config.resolve_lookahead_size(),
        compact_thresh: u32::MAX,
        read_buffer: read_buf.as_mut_ptr() as *mut c_void,
        prog_buffer: prog_buf.as_mut_ptr() as *mut c_void,
        lookahead_buffer: lookahead_buf.as_mut_ptr() as *mut c_void,
        name_max: config.name_max,
        file_max: config.file_max,
        attr_max: config.attr_max,
        metadata_max: 0,
        inline_max: 0,
    };

    BorrowedFsInner {
        lfs: MaybeUninit::zeroed(),
        config: lfs_config,
        storage,
        _read_buf: read_buf,
        _prog_buf: prog_buf,
        _lookahead_buf: lookahead_buf,
    }
}

fn wire_context_borrowed<S: Storage>(inner: &mut BorrowedFsInner<'_, S>) {
    inner.config.context = inner.storage as *mut S as *mut c_void;
    inner.config.read_buffer = inner._read_buf.as_mut_ptr() as *mut c_void;
    inner.config.prog_buffer = inner._prog_buf.as_mut_ptr() as *mut c_void;
    inner.config.lookahead_buffer = inner._lookahead_buf.as_mut_ptr() as *mut c_void;
}

fn null_terminate(s: &str) -> Vec<u8> {
    let mut v: Vec<u8> = s.bytes().collect();
    v.push(0);
    v
}
