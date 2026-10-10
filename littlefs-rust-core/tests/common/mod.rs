//! Shared test helpers for littlefs-rust-core integration tests.
//!
//! Reduces duplication across test files. Provides RAM block device, config builder,
//! and assert helpers. Upstream reference: tests/test_superblocks.toml

#![allow(dead_code)]

pub mod dump;
pub mod powerloss;

use core::cell::RefCell;
use littlefs_rust_core::{LfsConfig, LFS_ERR_CORRUPT};

/// Initialize env_logger for tests that use logging. Idempotent.
pub fn init_logger() {
    let _ = env_logger::try_init();
}

/// Magic string "littlefs" in superblock blocks. Per lfs.h.
/// Layout: [rev 4][CREATE tag 4]["littlefs" 8] — C format_and_dump shows magic at 8
pub const MAGIC: &[u8; 8] = b"littlefs";
pub const MAGIC_OFFSET: u32 = 8;

/// RAM block device storage. Erase = 0xff; prog = copy; read = copy.
pub struct RamStorage {
    pub data: Vec<u8>,
    pub block_size: u32,
    pub block_count: u32,
    /// Panic on a prog to bytes that are not erased (0xff), as upstream
    /// lfs_emubd does. Off by default; some tests rewrite blocks in place.
    pub assert_erased_prog: bool,
}

impl RamStorage {
    pub fn new(block_size: u32, block_count: u32) -> Self {
        let size = (block_size as usize)
            .checked_mul(block_count as usize)
            .expect("overflow");
        Self {
            data: vec![0u8; size],
            block_size,
            block_count,
            assert_erased_prog: false,
        }
    }

    pub fn block_offset(&self, block: u32) -> usize {
        (block as usize)
            .checked_mul(self.block_size as usize)
            .expect("block overflow")
    }

    pub fn read(&mut self, block: u32, off: u32, buf: &mut [u8]) {
        let base = self.block_offset(block);
        let start = base + off as usize;
        let end = start + buf.len();
        buf.copy_from_slice(&self.data[start..end]);
    }

    pub fn prog(&mut self, block: u32, off: u32, buf: &[u8]) {
        let base = self.block_offset(block);
        let start = base + off as usize;
        let end = start + buf.len();
        if self.assert_erased_prog {
            if let Some(i) = self.data[start..end].iter().position(|&b| b != 0xff) {
                panic!(
                    "prog to non-erased byte: block {block} off {}",
                    off as usize + i
                );
            }
        }
        self.data[start..end].copy_from_slice(buf);
    }

    pub fn erase(&mut self, block: u32) {
        let base = self.block_offset(block);
        let end = base + self.block_size as usize;
        self.data[base..end].fill(0xff);
    }
}

unsafe extern "C" fn ram_read(
    cfg: *const LfsConfig,
    block: u32,
    off: u32,
    buffer: *mut u8,
    size: u32,
) -> i32 {
    let ctx = (*cfg).context as *mut RamStorage;
    let ram = &mut *ctx;
    let size = size as usize;
    let buf = core::slice::from_raw_parts_mut(buffer, size);
    ram.read(block, off, buf);
    0
}

unsafe extern "C" fn ram_prog(
    cfg: *const LfsConfig,
    block: u32,
    off: u32,
    buffer: *const u8,
    size: u32,
) -> i32 {
    let ctx = (*cfg).context as *mut RamStorage;
    let ram = &mut *ctx;
    let size = size as usize;
    let buf = core::slice::from_raw_parts(buffer, size);
    ram.prog(block, off, buf);
    0
}

unsafe extern "C" fn ram_erase(cfg: *const LfsConfig, block: u32) -> i32 {
    let ctx = (*cfg).context as *mut RamStorage;
    let ram = &mut *ctx;
    ram.erase(block);
    0
}

unsafe extern "C" fn ram_sync(_cfg: *const LfsConfig) -> i32 {
    0
}

/// Mode determining how "bad-blocks" behave during testing.
/// Matches C lfs_emubd_badblock_behavior_t exactly.
///
/// C: reference/bd/lfs_emubd.h:38-44
/// ```c
/// typedef enum lfs_emubd_badblock_behavior {
///     LFS_EMUBD_BADBLOCK_PROGERROR  = 0, // Error on prog
///     LFS_EMUBD_BADBLOCK_ERASEERROR = 1, // Error on erase
///     LFS_EMUBD_BADBLOCK_READERROR  = 2, // Error on read
///     LFS_EMUBD_BADBLOCK_PROGNOOP   = 3, // Prog does nothing silently
///     LFS_EMUBD_BADBLOCK_ERASENOOP  = 4, // Erase does nothing silently
/// } lfs_emubd_badblock_behavior_t;
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u32)]
pub enum BadBlockBehavior {
    ProgError = 0,
    EraseError = 1,
    ReadError = 2,
    ProgNoop = 3,
    EraseNoop = 4,
}

/// RAM storage with bad-block simulation. Wraps RamStorage and applies
/// BadBlockBehavior in the appropriate callback when a block is marked bad.
///
/// Matches the bad-block logic in C lfs_emubd_read/prog/erase
/// (reference/bd/lfs_emubd.c:287-525).
pub struct BadBlockRamStorage {
    pub ram: RamStorage,
    pub bad_blocks: RefCell<Vec<u32>>,
    pub behavior: BadBlockBehavior,
}

impl BadBlockRamStorage {
    pub fn new(block_size: u32, block_count: u32) -> Self {
        Self {
            ram: RamStorage::new(block_size, block_count),
            bad_blocks: RefCell::new(Vec::new()),
            behavior: BadBlockBehavior::ProgError,
        }
    }

    pub fn new_with_behavior(
        block_size: u32,
        block_count: u32,
        behavior: BadBlockBehavior,
    ) -> Self {
        Self {
            ram: RamStorage::new(block_size, block_count),
            bad_blocks: RefCell::new(Vec::new()),
            behavior,
        }
    }

    pub fn set_bad_block(&self, block: u32) {
        let mut blocks = self.bad_blocks.borrow_mut();
        if !blocks.contains(&block) {
            blocks.push(block);
        }
    }

    pub fn clear_bad_block(&self, block: u32) {
        self.bad_blocks.borrow_mut().retain(|&b| b != block);
    }

    fn is_bad(&self, block: u32) -> bool {
        self.bad_blocks.borrow().contains(&block)
    }
}

/// C: lfs_emubd_read — block bad check (lfs_emubd.c:303-308)
/// Only READERROR triggers on read; all other behaviors allow the read through.
unsafe extern "C" fn badblock_read(
    cfg: *const LfsConfig,
    block: u32,
    off: u32,
    buffer: *mut u8,
    size: u32,
) -> i32 {
    let ctx = (*cfg).context as *mut BadBlockRamStorage;
    let badblock = &mut *ctx;
    if badblock.is_bad(block) && badblock.behavior == BadBlockBehavior::ReadError {
        return LFS_ERR_CORRUPT;
    }
    let size = size as usize;
    let buf = core::slice::from_raw_parts_mut(buffer, size);
    badblock.ram.read(block, off, buf);
    0
}

/// C: lfs_emubd_prog — block bad check (lfs_emubd.c:358-370)
/// PROGERROR → return LFS_ERR_CORRUPT
/// PROGNOOP or ERASENOOP → return 0 (silently skip the prog)
/// All others → prog normally
unsafe extern "C" fn badblock_prog(
    cfg: *const LfsConfig,
    block: u32,
    off: u32,
    buffer: *const u8,
    size: u32,
) -> i32 {
    let ctx = (*cfg).context as *mut BadBlockRamStorage;
    let badblock = &mut *ctx;
    if badblock.is_bad(block) {
        match badblock.behavior {
            BadBlockBehavior::ProgError => return LFS_ERR_CORRUPT,
            BadBlockBehavior::ProgNoop | BadBlockBehavior::EraseNoop => return 0,
            _ => {}
        }
    }
    let size = size as usize;
    let buf = core::slice::from_raw_parts(buffer, size);
    badblock.ram.prog(block, off, buf);
    0
}

/// C: lfs_emubd_erase — block bad check (lfs_emubd.c:454-468)
/// ERASEERROR → return LFS_ERR_CORRUPT
/// ERASENOOP → return 0 (silently skip the erase)
/// All others → erase normally
unsafe extern "C" fn badblock_erase(cfg: *const LfsConfig, block: u32) -> i32 {
    let ctx = (*cfg).context as *mut BadBlockRamStorage;
    let badblock = &mut *ctx;
    if badblock.is_bad(block) {
        match badblock.behavior {
            BadBlockBehavior::EraseError => return LFS_ERR_CORRUPT,
            BadBlockBehavior::EraseNoop => return 0,
            _ => {}
        }
    }
    badblock.ram.erase(block);
    0
}

unsafe extern "C" fn badblock_sync(_cfg: *const LfsConfig) -> i32 {
    0
}

/// Holds RAM storage, config, and buffers. Keeps pointers valid for lfs_* calls.
pub struct TestEnv {
    pub ram: RamStorage,
    pub config: LfsConfig,
    pub _read_buf: Vec<u8>,
    pub _prog_buf: Vec<u8>,
    pub _lookahead_buf: Vec<u8>,
}

/// Default block size. Matches upstream.
const BLOCK_SIZE: u32 = 512;

/// Build test environment with custom cache_size. Used for spill tests (e.g. cache 512).
pub fn config_with_cache(cache_size: u32, block_count: u32) -> TestEnv {
    let mut env = default_config(block_count);
    env.config.cache_size = cache_size;
    env
}

/// Build test environment with custom geometry. For geometry-specific tests (block_size=512, block_count=1024).
pub fn config_with_geometry(block_size: u32, block_count: u32) -> TestEnv {
    let ram = RamStorage::new(block_size, block_count);
    let read_buf = vec![0u8; block_size as usize];
    let prog_buf = vec![0u8; block_size as usize];
    let lookahead_buf = vec![0u8; block_size as usize];

    let config = LfsConfig {
        context: core::ptr::null_mut(),
        read: Some(ram_read),
        prog: Some(ram_prog),
        erase: Some(ram_erase),
        sync: Some(ram_sync),
        read_size: 16,
        prog_size: 16,
        block_size,
        block_count,
        block_cycles: -1,
        cache_size: block_size,
        lookahead_size: block_size,
        compact_thresh: u32::MAX,
        read_buffer: read_buf.as_ptr() as *mut core::ffi::c_void,
        prog_buffer: prog_buf.as_ptr() as *mut core::ffi::c_void,
        lookahead_buffer: lookahead_buf.as_ptr() as *mut core::ffi::c_void,
        name_max: 255,
        file_max: 2_147_483_647,
        attr_max: 1022,
        metadata_max: 0,
        inline_max: 0,
    };

    let mut env = TestEnv {
        ram,
        config,
        _read_buf: read_buf,
        _prog_buf: prog_buf,
        _lookahead_buf: lookahead_buf,
    };
    env.config.read_buffer = env._read_buf.as_mut_ptr() as *mut core::ffi::c_void;
    env.config.prog_buffer = env._prog_buf.as_mut_ptr() as *mut core::ffi::c_void;
    env.config.lookahead_buffer = env._lookahead_buf.as_mut_ptr() as *mut core::ffi::c_void;
    env
}

/// Build test environment with RAM BD. block_count defaults to 128 (upstream).
pub fn default_config(block_count: u32) -> TestEnv {
    let block_size = BLOCK_SIZE;
    let ram = RamStorage::new(block_size, block_count);
    let read_buf = vec![0u8; block_size as usize];
    let prog_buf = vec![0u8; block_size as usize];
    let lookahead_buf = vec![0u8; block_size as usize];

    let config = LfsConfig {
        context: core::ptr::null_mut(),
        read: Some(ram_read),
        prog: Some(ram_prog),
        erase: Some(ram_erase),
        sync: Some(ram_sync),
        read_size: 16,
        prog_size: 16,
        block_size,
        block_count,
        block_cycles: -1,
        cache_size: block_size,
        lookahead_size: block_size,
        compact_thresh: u32::MAX, // -1 in C
        read_buffer: read_buf.as_ptr() as *mut core::ffi::c_void,
        prog_buffer: prog_buf.as_ptr() as *mut core::ffi::c_void,
        lookahead_buffer: lookahead_buf.as_ptr() as *mut core::ffi::c_void,
        name_max: 255,
        file_max: 2_147_483_647,
        attr_max: 1022,
        metadata_max: 0,
        inline_max: 0,
    };

    // Build TestEnv, then set context/buffers. We must set context after returning
    // to the caller, since env moves and the stored &mut env.ram would otherwise
    // be a dangling pointer. Use TestEnv::init_context() after default_config().
    let mut env = TestEnv {
        ram,
        config,
        _read_buf: read_buf,
        _prog_buf: prog_buf,
        _lookahead_buf: lookahead_buf,
    };
    env.config.read_buffer = env._read_buf.as_mut_ptr() as *mut core::ffi::c_void;
    env.config.prog_buffer = env._prog_buf.as_mut_ptr() as *mut core::ffi::c_void;
    env.config.lookahead_buffer = env._lookahead_buf.as_mut_ptr() as *mut core::ffi::c_void;
    env
}

/// Call after default_config() to set context to ram. Required because context
/// must point to env.ram at its final address (after the env has been moved).
pub fn init_context(env: &mut TestEnv) {
    env.config.context = &mut env.ram as *mut RamStorage as *mut core::ffi::c_void;
}

/// A second config that shares the same RAM device but has its own buffers
/// and a different block_count. For lfs_fs_grow shrink tests where the
/// reduced-size config must mount the same underlying storage.
///
/// C pattern: `struct lfs_config cfg2 = *cfg; cfg2.block_count = N;`
pub struct ClonedConfig {
    pub config: LfsConfig,
    pub _read_buf: Vec<u8>,
    pub _prog_buf: Vec<u8>,
    pub _lookahead_buf: Vec<u8>,
}

/// Clone a TestEnv's config with a different block_count, sharing the same
/// RAM context pointer. Caller must ensure the original TestEnv outlives this.
pub fn clone_config_with_block_count(env: &TestEnv, block_count: u32) -> ClonedConfig {
    let bs = env.config.block_size as usize;
    let mut read_buf = vec![0u8; bs];
    let mut prog_buf = vec![0u8; bs];
    let mut lookahead_buf = vec![0u8; bs];
    let config = LfsConfig {
        context: env.config.context,
        read: env.config.read,
        prog: env.config.prog,
        erase: env.config.erase,
        sync: env.config.sync,
        read_size: env.config.read_size,
        prog_size: env.config.prog_size,
        block_size: env.config.block_size,
        block_count,
        block_cycles: env.config.block_cycles,
        cache_size: env.config.cache_size,
        lookahead_size: env.config.lookahead_size,
        compact_thresh: env.config.compact_thresh,
        read_buffer: read_buf.as_mut_ptr() as *mut core::ffi::c_void,
        prog_buffer: prog_buf.as_mut_ptr() as *mut core::ffi::c_void,
        lookahead_buffer: lookahead_buf.as_mut_ptr() as *mut core::ffi::c_void,
        name_max: env.config.name_max,
        file_max: env.config.file_max,
        attr_max: env.config.attr_max,
        metadata_max: env.config.metadata_max,
        inline_max: env.config.inline_max,
    };
    ClonedConfig {
        config,
        _read_buf: read_buf,
        _prog_buf: prog_buf,
        _lookahead_buf: lookahead_buf,
    }
}

/// TestEnv variant with bad-block BD. Use for test_alloc_bad_blocks.
pub struct BadBlockTestEnv {
    pub badblock_ram: BadBlockRamStorage,
    pub config: LfsConfig,
    pub _read_buf: Vec<u8>,
    pub _prog_buf: Vec<u8>,
    pub _lookahead_buf: Vec<u8>,
}

/// Build test environment with bad-block BD. Default behavior is ProgError
/// (matching C LFS_EMUBD_BADBLOCK_PROGERROR, the upstream default).
pub fn config_badblock(block_count: u32) -> BadBlockTestEnv {
    config_badblock_with_behavior(block_count, BadBlockBehavior::ProgError)
}

/// Build test environment with bad-block BD and explicit behavior.
pub fn config_badblock_with_behavior(
    block_count: u32,
    behavior: BadBlockBehavior,
) -> BadBlockTestEnv {
    let block_size = BLOCK_SIZE;
    let badblock_ram = BadBlockRamStorage::new_with_behavior(block_size, block_count, behavior);
    let read_buf = vec![0u8; block_size as usize];
    let prog_buf = vec![0u8; block_size as usize];
    let lookahead_buf = vec![0u8; block_size as usize];

    let config = LfsConfig {
        context: core::ptr::null_mut(),
        read: Some(badblock_read),
        prog: Some(badblock_prog),
        erase: Some(badblock_erase),
        sync: Some(badblock_sync),
        read_size: 16,
        prog_size: 16,
        block_size,
        block_count,
        block_cycles: -1,
        cache_size: block_size,
        lookahead_size: block_size,
        compact_thresh: u32::MAX,
        read_buffer: read_buf.as_ptr() as *mut core::ffi::c_void,
        prog_buffer: prog_buf.as_ptr() as *mut core::ffi::c_void,
        lookahead_buffer: lookahead_buf.as_ptr() as *mut core::ffi::c_void,
        name_max: 255,
        file_max: 2_147_483_647,
        attr_max: 1022,
        metadata_max: 0,
        inline_max: 0,
    };

    let mut env = BadBlockTestEnv {
        badblock_ram,
        config,
        _read_buf: read_buf,
        _prog_buf: prog_buf,
        _lookahead_buf: lookahead_buf,
    };
    env.config.read_buffer = env._read_buf.as_mut_ptr() as *mut core::ffi::c_void;
    env.config.prog_buffer = env._prog_buf.as_mut_ptr() as *mut core::ffi::c_void;
    env.config.lookahead_buffer = env._lookahead_buf.as_mut_ptr() as *mut core::ffi::c_void;
    env
}

/// Call after config_badblock() to set context. Required for BadBlockTestEnv.
pub fn init_badblock_context(env: &mut BadBlockTestEnv) {
    env.config.context = &mut env.badblock_ram as *mut BadBlockRamStorage as *mut core::ffi::c_void;
}

/// Run `f` with a process-level timeout. If the closure does not complete within
/// `secs` seconds, the process is aborted. Use for tests that may hang (e.g.
/// infinite loops in write paths).
pub fn run_with_timeout<F, R>(secs: u64, f: F) -> R
where
    F: FnOnce() -> R,
{
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let guard =
        std::thread::spawn(
            move || match rx.recv_timeout(std::time::Duration::from_secs(secs)) {
                Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    eprintln!("test exceeded {} second timeout, aborting", secs);
                    std::process::abort();
                }
            },
        );
    let result = f();
    let _ = tx.send(());
    guard.join().expect("timeout guard thread panicked");
    result
}

/// Panic if result is not 0.
pub fn assert_ok(result: i32) {
    if result != 0 {
        panic!("expected 0, got {}", result);
    }
}

/// Panic if result is not 0, with step name for debugging.
pub fn assert_ok_at(step: &str, result: i32) {
    if result != 0 {
        panic!("{} failed: {} (expected 0)", step, result);
    }
}

/// Panic if actual is not expected error code.
pub fn assert_err(expected: i32, actual: i32) {
    if actual != expected {
        panic!("expected error {}, got {}", expected, actual);
    }
}

/// Check if block has "littlefs" at offset 8 or 12 (layout varies by commit path).
fn block_has_magic(config: *const LfsConfig, block: u32) -> bool {
    let mut buf = [0u8; 24];
    let err = unsafe {
        let read = (*config).read.expect("read callback");
        read(config, block, 0, buf.as_mut_ptr(), 24)
    };
    if err != 0 {
        return false;
    }
    &buf[8..16] == MAGIC || &buf[12..20] == MAGIC
}

/// Assert "littlefs" in blocks 0 and 1 (at offset 8 or 12 depending on commit path).
/// Both blocks must have magic per upstream.
pub fn assert_superblock_magic(config: *const LfsConfig) {
    let has_0 = block_has_magic(config, 0);
    let has_1 = block_has_magic(config, 1);
    assert!(
        has_0 && has_1,
        "both blocks 0 and 1 must have MAGIC: block 0={} block 1={}",
        has_0,
        has_1
    );
}

/// Invoke config read callback for raw block access (e.g. test_superblocks_magic).
pub fn read_block_raw(config: *const LfsConfig, block: u32, off: u32, buf: &mut [u8]) -> i32 {
    unsafe {
        let read = (*config).read.expect("read callback");
        read(config, block, off, buf.as_mut_ptr(), buf.len() as u32)
    }
}

/// Invoke config prog callback for raw block write, bypassing the FS.
/// Mirrors read_block_raw but for writes. Used for corruption injection (test_evil).
///
/// C: lfs_emubd_prog via cfg->prog callback
pub fn write_block_raw(config: *const LfsConfig, block: u32, off: u32, data: &[u8]) -> i32 {
    unsafe {
        let prog = (*config).prog.expect("prog callback");
        prog(config, block, off, data.as_ptr(), data.len() as u32)
    }
}

/// Invoke config erase callback for raw block erase, bypassing the FS.
/// Used for corruption injection (test_evil_invalid_ctz_pointer).
///
/// C: cfg->erase(cfg, block)
pub fn erase_block_raw(config: *const LfsConfig, block: u32) -> i32 {
    unsafe {
        let erase = (*config).erase.expect("erase callback");
        erase(config, block)
    }
}

/// Pretty-print first `len` bytes of block for inspection. Used when debugging layout.
pub fn dump_block_hex(block: &[u8], label: &str, len: usize) {
    let len = len.min(block.len());
    eprintln!("{} (first {} bytes):", label, len);
    for (i, chunk) in block[..len].chunks(16).enumerate() {
        let hex: String = chunk
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<Vec<_>>()
            .join(" ");
        let ascii: String = chunk
            .iter()
            .map(|&b| {
                if b.is_ascii_graphic() || b == b' ' {
                    b as char
                } else {
                    '.'
                }
            })
            .collect();
        eprintln!("  {:04x}  {}  |{}|", i * 16, hex, ascii);
    }
}

/// Null-terminated path for lfs_* calls. Caller keeps vec in scope while using pointer.
pub fn path_bytes(s: &str) -> Vec<u8> {
    let mut v: Vec<u8> = s.bytes().collect();
    v.push(0);
    v
}

/// Read directory entry names (excluding "." and "..") from path. For use in dir tests.
pub fn dir_entry_names(
    lfs: *mut littlefs_rust_core::Lfs,
    _config: *const LfsConfig,
    path_str: &str,
) -> Result<Vec<String>, i32> {
    use littlefs_rust_core::{lfs_dir_close, lfs_dir_open, lfs_dir_read, LfsDir, LfsInfo};

    let path = path_bytes(path_str);
    let mut dir = core::mem::MaybeUninit::<LfsDir>::zeroed();
    let err = lfs_dir_open(lfs, dir.as_mut_ptr(), path.as_ptr());
    if err != 0 {
        return Err(err);
    }

    let mut names = Vec::new();
    let mut info = core::mem::MaybeUninit::<LfsInfo>::zeroed();
    loop {
        let n = lfs_dir_read(lfs, dir.as_mut_ptr(), info.as_mut_ptr());
        if n == 0 {
            break;
        }
        if n < 0 {
            let _ = lfs_dir_close(lfs, dir.as_mut_ptr());
            return Err(n);
        }
        let info_ref = unsafe { &*info.as_ptr() };
        let nul = info_ref.name.iter().position(|&b| b == 0).unwrap_or(256);
        let name = core::str::from_utf8(&info_ref.name[..nul])
            .unwrap_or("")
            .to_string();
        if name != "." && name != ".." {
            names.push(name);
        }
    }
    let _ = lfs_dir_close(lfs, dir.as_mut_ptr());
    Ok(names)
}

/// Open flags for lfs_file_open. Per lfs.h LFS_O_*.
pub const LFS_O_RDONLY: i32 = 1;
pub const LFS_O_WRONLY: i32 = 2;
pub const LFS_O_RDWR: i32 = 3;
pub const LFS_O_CREAT: i32 = 0x0100;
pub const LFS_O_EXCL: i32 = 0x0200;
pub const LFS_O_TRUNC: i32 = 0x0400;
pub const LFS_O_APPEND: i32 = 0x0800;

/// Seek whence. Per lfs.h enum lfs_whence_flags.
pub const LFS_SEEK_SET: i32 = 0;
pub const LFS_SEEK_CUR: i32 = 1;
pub const LFS_SEEK_END: i32 = 2;

/// Max file size. Per lfs.h LFS_FILE_MAX. lfs_soff_t is i32.
pub const LFS_FILE_MAX: i32 = 2_147_483_647;

/// Format, mount, create "hello" file with "Hello World!\0", unmount.
/// Returns env. Caller mounts again before reading.
pub fn fs_with_hello(env: &mut TestEnv) -> Result<(), i32> {
    use littlefs_rust_core::{
        lfs_file_close, lfs_file_open, lfs_file_write, lfs_format, lfs_mount, lfs_unmount, Lfs,
        LfsConfig, LfsFile,
    };

    init_context(env);
    let mut lfs = core::mem::MaybeUninit::<Lfs>::zeroed();
    let err = lfs_format(lfs.as_mut_ptr(), &env.config as *const LfsConfig);
    if err != 0 {
        return Err(err);
    }
    let err = lfs_mount(lfs.as_mut_ptr(), &env.config as *const LfsConfig);
    if err != 0 {
        return Err(err);
    }

    let path = path_bytes("hello");
    let data = b"Hello World!\0";
    let mut file = core::mem::MaybeUninit::<LfsFile>::zeroed();
    let err = lfs_file_open(
        lfs.as_mut_ptr(),
        file.as_mut_ptr(),
        path.as_ptr(),
        0x0100 | 2,
    );
    if err != 0 {
        let _ = lfs_unmount(lfs.as_mut_ptr());
        return Err(err);
    }
    let n = lfs_file_write(
        lfs.as_mut_ptr(),
        file.as_mut_ptr(),
        data.as_ptr() as *const core::ffi::c_void,
        data.len() as u32,
    );
    if n != data.len() as i32 {
        let _ = lfs_file_close(lfs.as_mut_ptr(), file.as_mut_ptr());
        let _ = lfs_unmount(lfs.as_mut_ptr());
        return Err(if n < 0 { n } else { -1 });
    }
    let err = lfs_file_close(lfs.as_mut_ptr(), file.as_mut_ptr());
    if err != 0 {
        let _ = lfs_unmount(lfs.as_mut_ptr());
        return Err(err);
    }
    let err = lfs_unmount(lfs.as_mut_ptr());
    if err != 0 {
        return Err(err);
    }
    Ok(())
}

/// Get the metadata block number (`m.pair[0]`) for a directory while mounted.
/// Caller must unmount before corrupting the returned block.
pub fn dir_block(lfs: *mut littlefs_rust_core::Lfs, dir_path: &str) -> u32 {
    dir_pair(lfs, dir_path)[0]
}

/// Get both metadata block numbers (`m.pair[0]`, `m.pair[1]`) for a directory while mounted.
/// Used by fix_relocation tests to set wear on dir pairs.
pub fn dir_pair(lfs: *mut littlefs_rust_core::Lfs, dir_path: &str) -> [u32; 2] {
    use littlefs_rust_core::{lfs_dir_close, lfs_dir_open, LfsDir};

    let path = path_bytes(dir_path);
    let mut dir = core::mem::MaybeUninit::<LfsDir>::zeroed();
    assert_ok(lfs_dir_open(lfs, dir.as_mut_ptr(), path.as_ptr()));
    let pair = unsafe { (*dir.as_ptr()).m.pair };
    assert_ok(lfs_dir_close(lfs, dir.as_mut_ptr()));
    [pair[0], pair[1]]
}

/// Corrupt a metadata block by overwriting 3 bytes near the last written byte.
/// Mirrors upstream test_move.toml corruption: find last non-erased (0xff) byte,
/// then set bytes [off-3..off] to 0x00 (BLOCK_SIZE & 0xff for BLOCK_SIZE=512).
/// Must be called while FS is unmounted.
pub fn corrupt_block(env: &mut TestEnv, block: u32) {
    let block_size = BLOCK_SIZE as usize;
    let mut buffer = vec![0u8; block_size];
    assert_eq!(
        read_block_raw(&env.config as *const LfsConfig, block, 0, &mut buffer),
        0
    );

    let mut off = block_size as i32 - 1;
    while off >= 0 && buffer[off as usize] == 0xff {
        off -= 1;
    }
    assert!(off >= 3, "block {block} has fewer than 4 written bytes");

    let start = (off - 3) as usize;
    buffer[start..start + 3].fill(0x00);

    env.ram.erase(block);
    env.ram.prog(block, 0, &buffer);
}

/// Build test environment with the given block_count and inline_max.
/// inline_max semantics match upstream C (lfs.c:4328-4347):
///   inline_max = -1  → (lfs_size_t)-1 = 0xFFFFFFFF → disabled (lfs uses 0)
///   inline_max = 0   → use library default (computed from cache/attr/metadata)
///   inline_max = N>0 → use N
pub fn config_with_inline_max(block_count: u32, inline_max: i32) -> TestEnv {
    let mut env = default_config(block_count);
    if inline_max < 0 {
        // C: (lfs_size_t)-1
        env.config.inline_max = u32::MAX;
    } else {
        env.config.inline_max = inline_max as u32;
    }
    env
}

/// Format fs, sync, return raw content of superblock blocks 0 and 1.
/// Helper for debug tests. Caller must init_context before.
pub fn format_and_read_superblock_blocks(env: &mut TestEnv) -> Result<(Vec<u8>, Vec<u8>), i32> {
    use littlefs_rust_core::{lfs_format, Lfs};

    let mut lfs = core::mem::MaybeUninit::<Lfs>::zeroed();
    let err = lfs_format(lfs.as_mut_ptr(), &env.config as *const LfsConfig);
    if err != 0 {
        return Err(err);
    }

    let block_size = env.config.block_size as usize;
    let mut block0 = vec![0u8; block_size];
    let mut block1 = vec![0u8; block_size];
    let err0 = read_block_raw(&env.config as *const LfsConfig, 0, 0, &mut block0);
    let err1 = read_block_raw(&env.config as *const LfsConfig, 1, 0, &mut block1);
    if err0 != 0 || err1 != 0 {
        return Err(if err0 != 0 { err0 } else { err1 });
    }
    Ok((block0, block1))
}

// ── Wear-leveling block device ──────────────────────────────────────────────
//
// For test_exhaustion. Wraps RamStorage with per-block erase-cycle tracking.
// Mirrors C lfs_emubd wear logic (reference/bd/lfs_emubd.c:287-525).
//
// erase_cycles = 0 means unlimited (no wear tracking).
// erase_cycles > 0: each erase increments wear[block]. When wear >= erase_cycles,
// the block is "bad" and behaves according to badblock_behavior.

/// Per-block erase-cycle tracking BD wrapper.
/// Mirrors C lfs_emubd with erase_cycles + badblock_behavior.
///
/// C: reference/bd/lfs_emubd.h:69-161
pub struct WearLevelingBd {
    pub ram: RamStorage,
    /// Max erase cycles per block before it goes bad. 0 = unlimited.
    pub erase_cycles: u32,
    /// How bad blocks behave. Default: ProgError (C default).
    pub badblock_behavior: BadBlockBehavior,
    /// Per-block erase count.
    pub wear: Vec<u32>,
    pub block_count: u32,
    /// What value to fill blocks with on erase. -1 = no fill (skip memset).
    /// C: lfs_emubd_config.erase_value
    pub erase_value: i32,
}

impl WearLevelingBd {
    pub fn new(block_count: u32, block_size: u32, erase_cycles: u32) -> Self {
        Self {
            ram: RamStorage::new(block_size, block_count),
            erase_cycles,
            badblock_behavior: BadBlockBehavior::ProgError,
            wear: vec![0u32; block_count as usize],
            block_count,
            erase_value: 0xff,
        }
    }

    pub fn new_with_behavior(
        block_count: u32,
        block_size: u32,
        erase_cycles: u32,
        behavior: BadBlockBehavior,
    ) -> Self {
        Self {
            ram: RamStorage::new(block_size, block_count),
            erase_cycles,
            badblock_behavior: behavior,
            wear: vec![0u32; block_count as usize],
            block_count,
            erase_value: 0xff,
        }
    }

    pub fn new_full(
        block_count: u32,
        block_size: u32,
        erase_cycles: u32,
        behavior: BadBlockBehavior,
        erase_value: i32,
    ) -> Self {
        Self {
            ram: RamStorage::new(block_size, block_count),
            erase_cycles,
            badblock_behavior: behavior,
            wear: vec![0u32; block_count as usize],
            block_count,
            erase_value,
        }
    }

    /// Returns true if block has exceeded its erase cycle limit.
    /// C: `bd->cfg->erase_cycles && b->wear >= bd->cfg->erase_cycles`
    pub fn is_worn(&self, block: u32) -> bool {
        self.erase_cycles > 0 && self.wear[block as usize] >= self.erase_cycles
    }

    /// Get wear count for a block. Mirrors C lfs_emubd_wear.
    pub fn get_wear(&self, block: u32) -> u32 {
        self.wear[block as usize]
    }

    /// Set a specific block's wear count (for test setup).
    /// Mirrors C lfs_emubd_setwear.
    pub fn set_wear(&mut self, block: u32, cycles: u32) {
        self.wear[block as usize] = cycles;
    }
}

/// C: lfs_emubd_read — wear check (lfs_emubd.c:303-308)
/// Only READERROR triggers on read for worn blocks.
unsafe extern "C" fn wear_read(
    cfg: *const LfsConfig,
    block: u32,
    off: u32,
    buffer: *mut u8,
    size: u32,
) -> i32 {
    let ctx = (*cfg).context as *mut WearLevelingBd;
    let bd = &mut *ctx;
    if bd.is_worn(block) && bd.badblock_behavior == BadBlockBehavior::ReadError {
        return LFS_ERR_CORRUPT;
    }
    let size = size as usize;
    let buf = core::slice::from_raw_parts_mut(buffer, size);
    bd.ram.read(block, off, buf);
    0
}

/// C: lfs_emubd_prog — wear check (lfs_emubd.c:358-370)
/// PROGERROR → LFS_ERR_CORRUPT
/// PROGNOOP or ERASENOOP → return 0 (skip prog)
unsafe extern "C" fn wear_prog(
    cfg: *const LfsConfig,
    block: u32,
    off: u32,
    buffer: *const u8,
    size: u32,
) -> i32 {
    let ctx = (*cfg).context as *mut WearLevelingBd;
    let bd = &mut *ctx;
    if bd.is_worn(block) {
        match bd.badblock_behavior {
            BadBlockBehavior::ProgError => return LFS_ERR_CORRUPT,
            BadBlockBehavior::ProgNoop | BadBlockBehavior::EraseNoop => return 0,
            _ => {}
        }
    }
    let size = size as usize;
    let buf = core::slice::from_raw_parts(buffer, size);
    bd.ram.prog(block, off, buf);
    0
}

/// C: lfs_emubd_erase — wear tracking + bad check (lfs_emubd.c:453-469)
/// If erase_cycles > 0 and block is worn:
///   ERASEERROR → LFS_ERR_CORRUPT
///   ERASENOOP → return 0 (skip erase)
/// If not worn: increment wear, then erase.
unsafe extern "C" fn wear_erase(cfg: *const LfsConfig, block: u32) -> i32 {
    let ctx = (*cfg).context as *mut WearLevelingBd;
    let bd = &mut *ctx;
    // C: if (bd->cfg->erase_cycles) { ... }
    if bd.erase_cycles > 0 {
        if bd.wear[block as usize] >= bd.erase_cycles {
            match bd.badblock_behavior {
                BadBlockBehavior::EraseError => return LFS_ERR_CORRUPT,
                BadBlockBehavior::EraseNoop => return 0,
                _ => {}
            }
        } else {
            bd.wear[block as usize] += 1;
        }
    }
    // C: if (bd->cfg->erase_value != -1) { memset(..., erase_value, ...); }
    if bd.erase_value != -1 {
        let base = bd.ram.block_offset(block);
        let end = base + bd.ram.block_size as usize;
        bd.ram.data[base..end].fill(bd.erase_value as u8);
    }
    0
}

unsafe extern "C" fn wear_sync(_cfg: *const LfsConfig) -> i32 {
    0
}

/// Test environment with wear-leveling BD. Owns WearLevelingBd, config, buffers.
pub struct WearLevelingEnv {
    pub bd: WearLevelingBd,
    pub config: LfsConfig,
    pub _read_buf: Vec<u8>,
    pub _prog_buf: Vec<u8>,
    pub _lookahead_buf: Vec<u8>,
}

/// Build wear-leveling test environment.
/// erase_cycles = max erases per block before it goes bad (0 = unlimited).
pub fn config_with_wear_leveling(block_count: u32, erase_cycles: u32) -> WearLevelingEnv {
    config_with_wear_leveling_behavior(block_count, erase_cycles, BadBlockBehavior::ProgError)
}

/// Build wear-leveling test environment with explicit bad-block behavior.
pub fn config_with_wear_leveling_behavior(
    block_count: u32,
    erase_cycles: u32,
    behavior: BadBlockBehavior,
) -> WearLevelingEnv {
    let block_size = BLOCK_SIZE;
    let bd = WearLevelingBd::new_with_behavior(block_count, block_size, erase_cycles, behavior);
    let read_buf = vec![0u8; block_size as usize];
    let prog_buf = vec![0u8; block_size as usize];
    let lookahead_buf = vec![0u8; block_size as usize];

    let config = LfsConfig {
        context: core::ptr::null_mut(),
        read: Some(wear_read),
        prog: Some(wear_prog),
        erase: Some(wear_erase),
        sync: Some(wear_sync),
        read_size: 16,
        prog_size: 16,
        block_size,
        block_count,
        block_cycles: -1,
        cache_size: block_size,
        lookahead_size: block_size,
        compact_thresh: u32::MAX,
        read_buffer: read_buf.as_ptr() as *mut core::ffi::c_void,
        prog_buffer: prog_buf.as_ptr() as *mut core::ffi::c_void,
        lookahead_buffer: lookahead_buf.as_ptr() as *mut core::ffi::c_void,
        name_max: 255,
        file_max: 2_147_483_647,
        attr_max: 1022,
        metadata_max: 0,
        inline_max: 0,
    };

    let mut env = WearLevelingEnv {
        bd,
        config,
        _read_buf: read_buf,
        _prog_buf: prog_buf,
        _lookahead_buf: lookahead_buf,
    };
    env.config.read_buffer = env._read_buf.as_mut_ptr() as *mut core::ffi::c_void;
    env.config.prog_buffer = env._prog_buf.as_mut_ptr() as *mut core::ffi::c_void;
    env.config.lookahead_buffer = env._lookahead_buf.as_mut_ptr() as *mut core::ffi::c_void;
    env
}

/// Build wear-leveling test environment with all parameters.
pub fn config_with_wear_leveling_full(
    block_count: u32,
    erase_cycles: u32,
    behavior: BadBlockBehavior,
    erase_value: i32,
) -> WearLevelingEnv {
    let block_size = BLOCK_SIZE;
    let bd = WearLevelingBd::new_full(block_count, block_size, erase_cycles, behavior, erase_value);
    let read_buf = vec![0u8; block_size as usize];
    let prog_buf = vec![0u8; block_size as usize];
    let lookahead_buf = vec![0u8; block_size as usize];

    let config = LfsConfig {
        context: core::ptr::null_mut(),
        read: Some(wear_read),
        prog: Some(wear_prog),
        erase: Some(wear_erase),
        sync: Some(wear_sync),
        read_size: 16,
        prog_size: 16,
        block_size,
        block_count,
        block_cycles: -1,
        cache_size: block_size,
        lookahead_size: block_size,
        compact_thresh: u32::MAX,
        read_buffer: read_buf.as_ptr() as *mut core::ffi::c_void,
        prog_buffer: prog_buf.as_ptr() as *mut core::ffi::c_void,
        lookahead_buffer: lookahead_buf.as_ptr() as *mut core::ffi::c_void,
        name_max: 255,
        file_max: 2_147_483_647,
        attr_max: 1022,
        metadata_max: 0,
        inline_max: 0,
    };

    let mut env = WearLevelingEnv {
        bd,
        config,
        _read_buf: read_buf,
        _prog_buf: prog_buf,
        _lookahead_buf: lookahead_buf,
    };
    env.config.read_buffer = env._read_buf.as_mut_ptr() as *mut core::ffi::c_void;
    env.config.prog_buffer = env._prog_buf.as_mut_ptr() as *mut core::ffi::c_void;
    env.config.lookahead_buffer = env._lookahead_buf.as_mut_ptr() as *mut core::ffi::c_void;
    env
}

/// Call after config_with_wear_leveling() to set context. Required for WearLevelingEnv.
pub fn init_wear_leveling_context(env: &mut WearLevelingEnv) {
    env.config.context = &mut env.bd as *mut WearLevelingBd as *mut core::ffi::c_void;
}

// ── PRNG and chunked I/O helpers ────────────────────────────────────────────

/// xorshift32 PRNG matching C littlefs TEST_PRNG exactly.
/// Deterministic; same seed produces same sequence as C.
///
/// C: reference/runners/test_runner.c:568-577
/// ```c
/// uint32_t test_prng(uint32_t *state) {
///     uint32_t x = *state;
///     x ^= x << 13;
///     x ^= x >> 17;
///     x ^= x << 5;
///     *state = x;
///     return x;
/// }
/// ```
pub fn test_prng(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

/// Advance PRNG state by `n` steps (call test_prng n times, discard results).
/// Used when skipping over previously-written PRNG data (e.g. rewrite/append tests).
///
/// C pattern: `for (lfs_size_t b = 0; b < skip; b++) { TEST_PRNG(&prng); }`
pub fn advance_prng(state: &mut u32, n: u32) {
    for _ in 0..n {
        test_prng(state);
    }
}

/// Write `size` bytes of PRNG data to an open file in `chunk_size` chunks.
/// PRNG seeded with `seed`. Returns total bytes written.
///
/// Matches the C pattern used in test_files_large, test_files_rewrite, etc:
/// ```c
/// uint32_t prng = 1;
/// uint8_t buffer[1024];
/// for (lfs_size_t i = 0; i < SIZE; i += CHUNKSIZE) {
///     lfs_size_t chunk = lfs_min(CHUNKSIZE, SIZE-i);
///     for (lfs_size_t b = 0; b < chunk; b++) {
///         buffer[b] = TEST_PRNG(&prng) & 0xff;
///     }
///     lfs_file_write(&lfs, &file, buffer, chunk) => chunk;
/// }
/// ```
pub fn write_prng_file(
    lfs: *mut littlefs_rust_core::Lfs,
    file: *mut littlefs_rust_core::LfsFile,
    size: u32,
    chunk_size: u32,
    seed: u32,
) -> u32 {
    let mut prng = seed;
    let mut buffer = [0u8; 1024];
    let mut i: u32 = 0;
    while i < size {
        let chunk = core::cmp::min(chunk_size, size - i);
        for slot in buffer[..chunk as usize].iter_mut() {
            *slot = (test_prng(&mut prng) & 0xff) as u8;
        }
        let n = littlefs_rust_core::lfs_file_write(
            lfs,
            file,
            buffer.as_ptr() as *const core::ffi::c_void,
            chunk,
        );
        assert_eq!(
            n, chunk as i32,
            "write_prng_file: expected {} bytes written at offset {}, got {}",
            chunk, i, n
        );
        i += chunk;
    }
    size
}

/// Like write_prng_file but returns Err on write failure (e.g. power-loss LFS_ERR_IO).
/// Use in power-loss tests where writes can legitimately fail.
pub fn write_prng_file_result(
    lfs: *mut littlefs_rust_core::Lfs,
    file: *mut littlefs_rust_core::LfsFile,
    size: u32,
    chunk_size: u32,
    seed: u32,
) -> Result<u32, i32> {
    let mut prng = seed;
    let mut buffer = [0u8; 1024];
    let mut i: u32 = 0;
    while i < size {
        let chunk = core::cmp::min(chunk_size, size - i);
        for slot in buffer[..chunk as usize].iter_mut() {
            *slot = (test_prng(&mut prng) & 0xff) as u8;
        }
        let n = littlefs_rust_core::lfs_file_write(
            lfs,
            file,
            buffer.as_ptr() as *const core::ffi::c_void,
            chunk,
        );
        if n < 0 {
            return Err(n);
        }
        if n != chunk as i32 {
            return Err(-1);
        }
        i += chunk;
    }
    Ok(size)
}

/// Read `size` bytes from an open file in `chunk_size` chunks and verify
/// against the same PRNG sequence (seeded with `seed`). Panics on mismatch.
///
/// Matches the C read+verify pattern:
/// ```c
/// prng = 1;
/// for (lfs_size_t i = 0; i < SIZE; i += CHUNKSIZE) {
///     lfs_size_t chunk = lfs_min(CHUNKSIZE, SIZE-i);
///     lfs_file_read(&lfs, &file, buffer, chunk) => chunk;
///     for (lfs_size_t b = 0; b < chunk; b++) {
///         assert(buffer[b] == (TEST_PRNG(&prng) & 0xff));
///     }
/// }
/// ```
pub fn verify_prng_file(
    lfs: *mut littlefs_rust_core::Lfs,
    file: *mut littlefs_rust_core::LfsFile,
    size: u32,
    chunk_size: u32,
    seed: u32,
) {
    let mut prng = seed;
    let mut buffer = [0u8; 1024];
    let mut i: u32 = 0;
    while i < size {
        let chunk = core::cmp::min(chunk_size, size - i);
        let n = littlefs_rust_core::lfs_file_read(
            lfs,
            file,
            buffer.as_mut_ptr() as *mut core::ffi::c_void,
            chunk,
        );
        assert_eq!(
            n, chunk as i32,
            "verify_prng_file: expected {} bytes read at offset {}, got {}",
            chunk, i, n
        );
        for (b, &actual) in buffer[..chunk as usize].iter().enumerate() {
            let expected = (test_prng(&mut prng) & 0xff) as u8;
            assert_eq!(
                actual, expected,
                "verify_prng_file: mismatch at byte {} (chunk offset {}), expected {:#04x}, got {:#04x}",
                i as usize + b, b, expected, actual
            );
        }
        i += chunk;
    }
}

/// Same as verify_prng_file but uses existing PRNG state (for verifying a tail after advance).
/// Used when reading SIZE2..SIZE1 in test_files_rewrite (PRNG was advanced by SIZE2 from seed 1).
pub fn verify_prng_file_with_state(
    lfs: *mut littlefs_rust_core::Lfs,
    file: *mut littlefs_rust_core::LfsFile,
    size: u32,
    chunk_size: u32,
    prng: &mut u32,
) {
    let mut buffer = [0u8; 1024];
    let mut i: u32 = 0;
    while i < size {
        let chunk = core::cmp::min(chunk_size, size - i);
        let n = littlefs_rust_core::lfs_file_read(
            lfs,
            file,
            buffer.as_mut_ptr() as *mut core::ffi::c_void,
            chunk,
        );
        assert_eq!(
            n, chunk as i32,
            "verify_prng_file_with_state: expected {} bytes read at offset {}, got {}",
            chunk, i, n
        );
        for (b, &actual) in buffer[..chunk as usize].iter().enumerate() {
            let expected = (test_prng(prng) & 0xff) as u8;
            assert_eq!(
                actual, expected,
                "verify_prng_file_with_state: mismatch at byte {} (chunk offset {}), expected {:#04x}, got {:#04x}",
                i as usize + b, b, expected, actual
            );
        }
        i += chunk;
    }
}
