//! Power-loss simulation tests.
//!
//! Upstream: tests/test_powerloss.toml
//! Source: https://github.com/littlefs-project/littlefs/blob/master/tests/test_powerloss.toml

mod common;

use common::{
    assert_ok_at, default_config, erase_block_raw, init_context, init_logger, path_bytes,
    powerloss::{
        init_powerloss_context, powerloss_config, powerloss_config_with_behavior,
        run_powerloss_exhaustive, run_powerloss_linear, run_powerloss_log, PowerLossBehavior,
    },
    read_block_raw, write_block_raw, LFS_O_APPEND, LFS_O_CREAT, LFS_O_RDONLY, LFS_O_WRONLY,
};
use littlefs_rust_core::{
    lfs_dir_close, lfs_dir_open, lfs_file_close, lfs_file_open, lfs_file_read, lfs_file_sync,
    lfs_file_write, lfs_format, lfs_mkdir, lfs_mount, lfs_unmount, Lfs, LfsConfig, LfsDir, LfsFile,
    LFS_ERR_IO,
};

// --- test_powerloss_only_rev ---
// Upstream: write rev+1 to one block of dir pair; mount picks higher rev, read/write still works.
#[test]
fn test_powerloss_only_rev() {
    init_logger();
    let mut env = default_config(128);
    init_context(&mut env);

    let mut lfs = core::mem::MaybeUninit::<Lfs>::zeroed();
    assert_ok_at(
        "format",
        lfs_format(lfs.as_mut_ptr(), &env.config as *const LfsConfig),
    );
    assert_ok_at(
        "mount",
        lfs_mount(lfs.as_mut_ptr(), &env.config as *const LfsConfig),
    );

    let lfs_ptr = lfs.as_mut_ptr();
    let path_nb = path_bytes("notebook");
    let path_paper = path_bytes("notebook/paper");
    assert_ok_at("mkdir notebook", lfs_mkdir(lfs_ptr, path_nb.as_ptr()));

    let mut file = core::mem::MaybeUninit::<LfsFile>::zeroed();
    assert_ok_at(
        "file_open paper create",
        lfs_file_open(
            lfs_ptr,
            file.as_mut_ptr(),
            path_paper.as_ptr(),
            LFS_O_WRONLY | LFS_O_CREAT | LFS_O_APPEND,
        ),
    );
    let buf = b"hello";
    for i in 0..5 {
        let n = lfs_file_write(
            lfs_ptr,
            file.as_mut_ptr(),
            buf.as_ptr() as *const core::ffi::c_void,
            buf.len() as u32,
        );
        assert!(n == buf.len() as i32);
        assert_ok_at(
            &format!("file_sync #{} (first loop)", i + 1),
            lfs_file_sync(lfs_ptr, file.as_mut_ptr()),
        );
    }
    assert_ok_at("file_close", lfs_file_close(lfs_ptr, file.as_mut_ptr()));

    let mut file = core::mem::MaybeUninit::<LfsFile>::zeroed();
    assert_ok_at(
        "file_open paper read",
        lfs_file_open(
            lfs_ptr,
            file.as_mut_ptr(),
            path_paper.as_ptr(),
            LFS_O_RDONLY,
        ),
    );
    let mut rbuf = [0u8; 256];
    for _ in 0..5 {
        let n = lfs_file_read(
            lfs_ptr,
            file.as_mut_ptr(),
            rbuf.as_mut_ptr() as *mut core::ffi::c_void,
            5,
        );
        assert_eq!(n, 5);
        assert_eq!(&rbuf[..5], b"hello");
    }
    assert_ok_at("file_close", lfs_file_close(lfs_ptr, file.as_mut_ptr()));
    assert_ok_at("unmount", lfs_unmount(lfs_ptr));

    // Get dir pair and rev from a fresh mount, then corrupt rev
    assert_ok_at(
        "mount before corrupt",
        lfs_mount(lfs_ptr, &env.config as *const LfsConfig),
    );
    let mut dir = core::mem::MaybeUninit::<LfsDir>::zeroed();
    assert_ok_at(
        "dir_open notebook",
        lfs_dir_open(lfs_ptr, dir.as_mut_ptr(), path_nb.as_ptr()),
    );
    let pair = unsafe { (*dir.as_ptr()).m.pair };
    let rev = unsafe { (*dir.as_ptr()).m.rev };
    assert_ok_at("dir_close", lfs_dir_close(lfs_ptr, dir.as_mut_ptr()));
    assert_ok_at("unmount before corrupt", lfs_unmount(lfs_ptr));

    // Partial write: rev+1 in block
    let block_size = env.config.block_size as usize;
    let mut block_buf = vec![0u8; block_size];
    let read_fn = env.config.read.expect("read");
    unsafe {
        read_fn(
            &env.config as *const LfsConfig,
            pair[1],
            0,
            block_buf.as_mut_ptr(),
            block_size as u32,
        );
    }
    block_buf[0..4].copy_from_slice(&(rev + 1).to_le_bytes());
    let erase_fn = env.config.erase.expect("erase");
    let prog_fn = env.config.prog.expect("prog");
    unsafe {
        erase_fn(&env.config as *const LfsConfig, pair[1]);
        prog_fn(
            &env.config as *const LfsConfig,
            pair[1],
            0,
            block_buf.as_ptr(),
            block_size as u32,
        );
    }

    assert_ok_at(
        "mount after corrupt",
        lfs_mount(lfs_ptr, &env.config as *const LfsConfig),
    );

    let mut file = core::mem::MaybeUninit::<LfsFile>::zeroed();
    assert_ok_at(
        "file_open paper read after corrupt",
        lfs_file_open(
            lfs_ptr,
            file.as_mut_ptr(),
            path_paper.as_ptr(),
            LFS_O_RDONLY,
        ),
    );
    for _ in 0..5 {
        let n = lfs_file_read(
            lfs_ptr,
            file.as_mut_ptr(),
            rbuf.as_mut_ptr() as *mut core::ffi::c_void,
            5,
        );
        assert_eq!(n, 5);
        assert_eq!(&rbuf[..5], b"hello");
    }
    assert_ok_at("file_close", lfs_file_close(lfs_ptr, file.as_mut_ptr()));

    let mut file = core::mem::MaybeUninit::<LfsFile>::zeroed();
    assert_ok_at(
        "file_open paper append",
        lfs_file_open(
            lfs_ptr,
            file.as_mut_ptr(),
            path_paper.as_ptr(),
            LFS_O_WRONLY | LFS_O_APPEND,
        ),
    );
    let buf2 = b"goodbye";
    for i in 0..5 {
        let n = lfs_file_write(
            lfs_ptr,
            file.as_mut_ptr(),
            buf2.as_ptr() as *const core::ffi::c_void,
            buf2.len() as u32,
        );
        assert!(n == buf2.len() as i32);
        assert_ok_at(
            &format!("file_sync #{} (after corrupt)", i + 1),
            lfs_file_sync(lfs_ptr, file.as_mut_ptr()),
        );
    }
    assert_ok_at("file_close", lfs_file_close(lfs_ptr, file.as_mut_ptr()));

    let mut file = core::mem::MaybeUninit::<LfsFile>::zeroed();
    assert_ok_at(
        "file_open paper read final",
        lfs_file_open(
            lfs_ptr,
            file.as_mut_ptr(),
            path_paper.as_ptr(),
            LFS_O_RDONLY,
        ),
    );
    for _ in 0..5 {
        let n = lfs_file_read(
            lfs_ptr,
            file.as_mut_ptr(),
            rbuf.as_mut_ptr() as *mut core::ffi::c_void,
            5,
        );
        assert_eq!(n, 5);
        assert_eq!(&rbuf[..5], b"hello");
    }
    for _ in 0..5 {
        let n = lfs_file_read(
            lfs_ptr,
            file.as_mut_ptr(),
            rbuf.as_mut_ptr() as *mut core::ffi::c_void,
            7,
        );
        assert_eq!(n, 7);
        assert_eq!(&rbuf[..7], b"goodbye");
    }
    assert_ok_at("file_close", lfs_file_close(lfs_ptr, file.as_mut_ptr()));
    assert_ok_at("unmount final", lfs_unmount(lfs_ptr));
}

// --- test_powerloss_trigger_first_write ---
// Unit test: fail_after_writes=1 causes first prog/erase to return LFS_ERR_IO.
#[test]
fn test_powerloss_trigger_first_write() {
    init_logger();
    let mut env = powerloss_config(128);
    init_powerloss_context(&mut env);
    env.set_fail_after_writes(1);

    let mut lfs = core::mem::MaybeUninit::<Lfs>::zeroed();
    let err = lfs_format(lfs.as_mut_ptr(), &env.config as *const LfsConfig);
    assert_eq!(
        err, LFS_ERR_IO,
        "format should fail on first write with fail_after_writes=1"
    );
}

// --- test_powerloss_runner_smoke ---
// Smoke test: run_powerloss_linear with mkdir op; verify mount works after power loss.
#[test]
fn test_powerloss_runner_smoke() {
    init_logger();
    let mut env = powerloss_config(128);
    init_powerloss_context(&mut env);

    let mut lfs = core::mem::MaybeUninit::<Lfs>::zeroed();
    assert_ok_at(
        "format",
        lfs_format(lfs.as_mut_ptr(), &env.config as *const LfsConfig),
    );
    let snapshot = env.snapshot();

    let path_d = path_bytes("d");
    let result = run_powerloss_linear(
        &mut env,
        &snapshot,
        64,
        |lfs_ptr, config| {
            let err = lfs_mount(lfs_ptr, config);
            if err != 0 {
                return Err(err);
            }
            let err = lfs_mkdir(lfs_ptr, path_d.as_ptr());
            if err != 0 {
                let _ = lfs_unmount(lfs_ptr);
                return Err(err);
            }
            let err = lfs_unmount(lfs_ptr);
            if err != 0 {
                return Err(err);
            }
            Ok(())
        },
        |lfs_ptr, config| {
            let err = lfs_mount(lfs_ptr, config);
            if err != 0 {
                return Err(err);
            }
            let _ = lfs_unmount(lfs_ptr);
            Ok(())
        },
    );
    result.expect("run_powerloss_linear should complete");
}

/// Upstream: [cases.test_powerloss_partial_prog]
/// defines.PROG_SIZE < BLOCK_SIZE, BYTE_OFF = [0, PROG_SIZE-1, PROG_SIZE/2], BYTE_VALUE = [0x33, 0xcc].
/// Imitate a partial prog: change one byte in the erased region where the next
/// commit to "notebook" would land. The fcrc of the last commit must no longer
/// match, so the next commit compacts instead of appending onto the garbage.
#[test]
fn test_powerloss_partial_prog() {
    init_logger();
    const PROG_SIZE: u32 = 16;
    let byte_offs: [u32; 3] = [0, PROG_SIZE - 1, PROG_SIZE / 2];
    let byte_values: [u8; 2] = [0x33, 0xcc];

    for &byte_off in &byte_offs {
        for &byte_value in &byte_values {
            let case = format!("off={byte_off} val=0x{byte_value:02x}");
            let mut env = default_config(128);
            init_context(&mut env);
            let cfg = &env.config as *const LfsConfig;
            assert_eq!(env.config.prog_size, PROG_SIZE);

            let mut lfs = core::mem::MaybeUninit::<Lfs>::zeroed();
            let lfs_ptr = lfs.as_mut_ptr();
            let path_nb = path_bytes("notebook");
            let path_paper = path_bytes("notebook/paper");
            assert_ok_at("format", lfs_format(lfs_ptr, cfg));

            assert_ok_at("mount", lfs_mount(lfs_ptr, cfg));
            assert_ok_at("mkdir notebook", lfs_mkdir(lfs_ptr, path_nb.as_ptr()));
            append_synced(lfs_ptr, &path_paper, LFS_O_CREAT, b"hello", 5);
            read_expect(lfs_ptr, &path_paper, &[(b"hello", 5)]);
            assert_ok_at("unmount", lfs_unmount(lfs_ptr));

            // get offset to next prog
            assert_ok_at("mount", lfs_mount(lfs_ptr, cfg));
            let mut dir = core::mem::MaybeUninit::<LfsDir>::zeroed();
            assert_ok_at(
                "dir_open notebook",
                lfs_dir_open(lfs_ptr, dir.as_mut_ptr(), path_nb.as_ptr()),
            );
            let block = unsafe { (*dir.as_ptr()).m.pair[0] };
            let off = unsafe { (*dir.as_ptr()).m.off };
            assert_ok_at("dir_close", lfs_dir_close(lfs_ptr, dir.as_mut_ptr()));
            assert_ok_at("unmount", lfs_unmount(lfs_ptr));

            // tweak byte
            let mut bbuffer = vec![0u8; env.config.block_size as usize];
            assert_ok_at("read block", read_block_raw(cfg, block, 0, &mut bbuffer));
            bbuffer[(off + byte_off) as usize] = byte_value;
            assert_ok_at("erase block", erase_block_raw(cfg, block));
            assert_ok_at("prog block", write_block_raw(cfg, block, 0, &bbuffer));

            // if littlefs doesn't notice the partial prog, the next commit
            // progs onto the tweaked byte and this panics
            env.ram.assert_erased_prog = true;

            assert_ok_at(
                &format!("mount after tweak {case}"),
                lfs_mount(lfs_ptr, cfg),
            );

            // can read?
            read_expect(lfs_ptr, &path_paper, &[(b"hello", 5)]);

            // can write?
            append_synced(lfs_ptr, &path_paper, 0, b"goodbye", 5);
            read_expect(lfs_ptr, &path_paper, &[(b"hello", 5), (b"goodbye", 5)]);

            assert_ok_at("unmount", lfs_unmount(lfs_ptr));
        }
    }
}

// --- test_powerloss_snapshot_restore ---
// Unit test: snapshot and restore preserve BD state.
#[test]
fn test_powerloss_snapshot_restore() {
    init_logger();
    let mut env = powerloss_config(128);
    init_powerloss_context(&mut env);

    let mut lfs = core::mem::MaybeUninit::<Lfs>::zeroed();
    assert_ok_at(
        "format",
        lfs_format(lfs.as_mut_ptr(), &env.config as *const LfsConfig),
    );
    let snapshot = env.snapshot();

    // Mutate ram
    env.ctx.ram.data[0] = 0;
    assert_ne!(env.ctx.ram.data[0], snapshot[0]);

    env.restore(&snapshot);
    assert_eq!(&env.ctx.ram.data[..], &snapshot[..]);

    assert_ok_at(
        "mount after restore",
        lfs_mount(lfs.as_mut_ptr(), &env.config as *const LfsConfig),
    );
    assert_ok_at("unmount", lfs_unmount(lfs.as_mut_ptr()));
}

// =============================================================================
// Debug tests. test_powerloss_only_rev / test_debug_powerloss_after_corrupt still
// fail with NOSPC on sync #5 after rev corruption; lfs_dir_split is now implemented.
// Remaining issue may be in compact/relocate when reading from corrupted block.
// =============================================================================

/// Minimal: file in root, write "hello" once, sync. No mkdir, no subdir.
#[test]
fn test_debug_file_root_single_write_sync() {
    init_logger();
    let mut env = default_config(128);
    init_context(&mut env);

    let mut lfs = core::mem::MaybeUninit::<Lfs>::zeroed();
    assert_ok_at(
        "format",
        lfs_format(lfs.as_mut_ptr(), &env.config as *const LfsConfig),
    );
    assert_ok_at(
        "mount",
        lfs_mount(lfs.as_mut_ptr(), &env.config as *const LfsConfig),
    );

    let lfs_ptr = lfs.as_mut_ptr();
    let path = path_bytes("paper");
    let mut file = core::mem::MaybeUninit::<LfsFile>::zeroed();
    assert_ok_at(
        "file_open create",
        lfs_file_open(
            lfs_ptr,
            file.as_mut_ptr(),
            path.as_ptr(),
            LFS_O_WRONLY | LFS_O_CREAT | LFS_O_APPEND,
        ),
    );
    let buf = b"hello";
    let n = lfs_file_write(
        lfs_ptr,
        file.as_mut_ptr(),
        buf.as_ptr() as *const core::ffi::c_void,
        buf.len() as u32,
    );
    assert_eq!(n, buf.len() as i32);
    assert_ok_at("file_sync", lfs_file_sync(lfs_ptr, file.as_mut_ptr()));
    assert_ok_at("file_close", lfs_file_close(lfs_ptr, file.as_mut_ptr()));
    assert_ok_at("unmount", lfs_unmount(lfs_ptr));
}

/// File in root, write "hello" 5x with sync each (like powerloss but no mkdir). Bisects root vs subdir.
#[test]
fn test_debug_file_root_repeated_write_sync() {
    init_logger();
    let mut env = default_config(128);
    init_context(&mut env);

    let mut lfs = core::mem::MaybeUninit::<Lfs>::zeroed();
    assert_ok_at(
        "format",
        lfs_format(lfs.as_mut_ptr(), &env.config as *const LfsConfig),
    );
    assert_ok_at(
        "mount",
        lfs_mount(lfs.as_mut_ptr(), &env.config as *const LfsConfig),
    );

    let lfs_ptr = lfs.as_mut_ptr();
    let path = path_bytes("paper");
    let mut file = core::mem::MaybeUninit::<LfsFile>::zeroed();
    assert_ok_at(
        "file_open create",
        lfs_file_open(
            lfs_ptr,
            file.as_mut_ptr(),
            path.as_ptr(),
            LFS_O_WRONLY | LFS_O_CREAT | LFS_O_APPEND,
        ),
    );
    let buf = b"hello";
    for i in 0..5 {
        let n = lfs_file_write(
            lfs_ptr,
            file.as_mut_ptr(),
            buf.as_ptr() as *const core::ffi::c_void,
            buf.len() as u32,
        );
        assert_eq!(n, buf.len() as i32);
        assert_ok_at(
            &format!("file_sync #{}", i + 1),
            lfs_file_sync(lfs_ptr, file.as_mut_ptr()),
        );
    }
    assert_ok_at("file_close", lfs_file_close(lfs_ptr, file.as_mut_ptr()));
    assert_ok_at("unmount", lfs_unmount(lfs_ptr));
}

/// Exact powerloss pattern (mkdir + file in subdir) but bisects which sync fails.
#[test]
fn test_debug_file_subdir_which_sync_fails() {
    init_logger();
    let mut env = default_config(128);
    init_context(&mut env);

    let mut lfs = core::mem::MaybeUninit::<Lfs>::zeroed();
    assert_ok_at(
        "format",
        lfs_format(lfs.as_mut_ptr(), &env.config as *const LfsConfig),
    );
    assert_ok_at(
        "mount",
        lfs_mount(lfs.as_mut_ptr(), &env.config as *const LfsConfig),
    );

    let lfs_ptr = lfs.as_mut_ptr();
    let path_nb = path_bytes("notebook");
    let path_paper = path_bytes("notebook/paper");
    assert_ok_at("mkdir notebook", lfs_mkdir(lfs_ptr, path_nb.as_ptr()));

    let mut file = core::mem::MaybeUninit::<LfsFile>::zeroed();
    assert_ok_at(
        "file_open paper create",
        lfs_file_open(
            lfs_ptr,
            file.as_mut_ptr(),
            path_paper.as_ptr(),
            LFS_O_WRONLY | LFS_O_CREAT | LFS_O_APPEND,
        ),
    );
    let buf = b"hello";
    for i in 0..5 {
        let n = lfs_file_write(
            lfs_ptr,
            file.as_mut_ptr(),
            buf.as_ptr() as *const core::ffi::c_void,
            buf.len() as u32,
        );
        assert_eq!(n, buf.len() as i32);
        let err = lfs_file_sync(lfs_ptr, file.as_mut_ptr());
        assert_ok_at(&format!("file_sync #{}", i + 1), err);
    }
    assert_ok_at("file_close", lfs_file_close(lfs_ptr, file.as_mut_ptr()));
    assert_ok_at("unmount", lfs_unmount(lfs_ptr));
}

/// Reproduces powerloss flow: setup, corrupt rev, then append. Bisects which sync fails after corrupt.
#[test]
fn test_debug_powerloss_after_corrupt_append() {
    init_logger();
    let mut env = default_config(128);
    init_context(&mut env);

    let mut lfs = core::mem::MaybeUninit::<Lfs>::zeroed();
    assert_ok_at(
        "format",
        lfs_format(lfs.as_mut_ptr(), &env.config as *const LfsConfig),
    );
    assert_ok_at(
        "mount",
        lfs_mount(lfs.as_mut_ptr(), &env.config as *const LfsConfig),
    );

    let lfs_ptr = lfs.as_mut_ptr();
    let path_nb = path_bytes("notebook");
    let path_paper = path_bytes("notebook/paper");
    assert_ok_at("mkdir notebook", lfs_mkdir(lfs_ptr, path_nb.as_ptr()));

    let mut file = core::mem::MaybeUninit::<LfsFile>::zeroed();
    assert_ok_at(
        "file_open paper create",
        lfs_file_open(
            lfs_ptr,
            file.as_mut_ptr(),
            path_paper.as_ptr(),
            LFS_O_WRONLY | LFS_O_CREAT | LFS_O_APPEND,
        ),
    );
    let buf = b"hello";
    for i in 0..5 {
        let n = lfs_file_write(
            lfs_ptr,
            file.as_mut_ptr(),
            buf.as_ptr() as *const core::ffi::c_void,
            buf.len() as u32,
        );
        assert_eq!(n, buf.len() as i32);
        assert_ok_at(
            &format!("file_sync #{}", i + 1),
            lfs_file_sync(lfs_ptr, file.as_mut_ptr()),
        );
    }
    assert_ok_at("file_close", lfs_file_close(lfs_ptr, file.as_mut_ptr()));
    assert_ok_at("unmount", lfs_unmount(lfs_ptr));

    assert_ok_at(
        "mount before corrupt",
        lfs_mount(lfs_ptr, &env.config as *const LfsConfig),
    );
    let mut dir = core::mem::MaybeUninit::<LfsDir>::zeroed();
    assert_ok_at(
        "dir_open notebook",
        lfs_dir_open(lfs_ptr, dir.as_mut_ptr(), path_nb.as_ptr()),
    );
    let pair = unsafe { (*dir.as_ptr()).m.pair };
    let rev = unsafe { (*dir.as_ptr()).m.rev };
    assert_ok_at("dir_close", lfs_dir_close(lfs_ptr, dir.as_mut_ptr()));
    assert_ok_at("unmount before corrupt", lfs_unmount(lfs_ptr));

    let block_size = env.config.block_size as usize;
    let mut block_buf = vec![0u8; block_size];
    let read_fn = env.config.read.expect("read");
    unsafe {
        read_fn(
            &env.config as *const LfsConfig,
            pair[1],
            0,
            block_buf.as_mut_ptr(),
            block_size as u32,
        );
    }
    block_buf[0..4].copy_from_slice(&(rev + 1).to_le_bytes());
    let erase_fn = env.config.erase.expect("erase");
    let prog_fn = env.config.prog.expect("prog");
    unsafe {
        erase_fn(&env.config as *const LfsConfig, pair[1]);
        prog_fn(
            &env.config as *const LfsConfig,
            pair[1],
            0,
            block_buf.as_ptr(),
            block_size as u32,
        );
    }

    assert_ok_at(
        "mount after corrupt",
        lfs_mount(lfs_ptr, &env.config as *const LfsConfig),
    );
    let mut file = core::mem::MaybeUninit::<LfsFile>::zeroed();
    assert_ok_at(
        "file_open paper append",
        lfs_file_open(
            lfs_ptr,
            file.as_mut_ptr(),
            path_paper.as_ptr(),
            LFS_O_WRONLY | LFS_O_APPEND,
        ),
    );
    let buf2 = b"goodbye";
    for i in 0..5 {
        let n = lfs_file_write(
            lfs_ptr,
            file.as_mut_ptr(),
            buf2.as_ptr() as *const core::ffi::c_void,
            buf2.len() as u32,
        );
        assert_eq!(n, buf2.len() as i32);
        assert_ok_at(
            &format!("file_sync #{} (after corrupt)", i + 1),
            lfs_file_sync(lfs_ptr, file.as_mut_ptr()),
        );
    }
    assert_ok_at("file_close", lfs_file_close(lfs_ptr, file.as_mut_ptr()));
    assert_ok_at("unmount", lfs_unmount(lfs_ptr));
}

// --- test_powerloss_runner_smoke_log ---
// Same as test_powerloss_runner_smoke but using run_powerloss_log (exponential stepping).
#[test]
fn test_powerloss_runner_smoke_log() {
    init_logger();
    let mut env = powerloss_config(128);
    init_powerloss_context(&mut env);

    let mut lfs = core::mem::MaybeUninit::<Lfs>::zeroed();
    assert_ok_at(
        "format",
        lfs_format(lfs.as_mut_ptr(), &env.config as *const LfsConfig),
    );
    let snapshot = env.snapshot();

    let path_d = path_bytes("d");
    let result = run_powerloss_log(
        &mut env,
        &snapshot,
        64,
        |lfs_ptr, config| {
            let err = lfs_mount(lfs_ptr, config);
            if err != 0 {
                return Err(err);
            }
            let err = lfs_mkdir(lfs_ptr, path_d.as_ptr());
            if err != 0 {
                let _ = lfs_unmount(lfs_ptr);
                return Err(err);
            }
            let err = lfs_unmount(lfs_ptr);
            if err != 0 {
                return Err(err);
            }
            Ok(())
        },
        |lfs_ptr, config| {
            let err = lfs_mount(lfs_ptr, config);
            if err != 0 {
                return Err(err);
            }
            let _ = lfs_unmount(lfs_ptr);
            Ok(())
        },
    );
    result.expect("run_powerloss_log should complete");
}

// --- test_powerloss_runner_smoke_exhaustive ---
// Same as test_powerloss_runner_smoke but using run_powerloss_exhaustive with depth=2.
#[test]
fn test_powerloss_runner_smoke_exhaustive() {
    init_logger();
    let mut env = powerloss_config(128);
    init_powerloss_context(&mut env);

    let mut lfs = core::mem::MaybeUninit::<Lfs>::zeroed();
    assert_ok_at(
        "format",
        lfs_format(lfs.as_mut_ptr(), &env.config as *const LfsConfig),
    );
    let snapshot = env.snapshot();

    let path_d = path_bytes("d");
    let result = run_powerloss_exhaustive(
        &mut env,
        &snapshot,
        64,
        2,
        |lfs_ptr, config| {
            let err = lfs_mount(lfs_ptr, config);
            if err != 0 {
                return Err(err);
            }
            let err = lfs_mkdir(lfs_ptr, path_d.as_ptr());
            if err != 0 {
                let _ = lfs_unmount(lfs_ptr);
                return Err(err);
            }
            let err = lfs_unmount(lfs_ptr);
            if err != 0 {
                return Err(err);
            }
            Ok(())
        },
        |lfs_ptr, config| {
            let err = lfs_mount(lfs_ptr, config);
            if err != 0 {
                return Err(err);
            }
            let _ = lfs_unmount(lfs_ptr);
            Ok(())
        },
    );
    result.expect("run_powerloss_exhaustive depth=2 should complete");
}

// --- test_powerloss_ooo_smoke ---
// OOO behaviour: writes between syncs may be reordered. Verify FS recovers correctly.
#[test]
fn test_powerloss_ooo_smoke() {
    init_logger();
    let mut env = powerloss_config_with_behavior(128, PowerLossBehavior::Ooo);
    init_powerloss_context(&mut env);

    let mut lfs = core::mem::MaybeUninit::<Lfs>::zeroed();
    assert_ok_at(
        "format",
        lfs_format(lfs.as_mut_ptr(), &env.config as *const LfsConfig),
    );
    let snapshot = env.snapshot();

    let path_d = path_bytes("d");
    let result = run_powerloss_linear(
        &mut env,
        &snapshot,
        64,
        |lfs_ptr, config| {
            let err = lfs_mount(lfs_ptr, config);
            if err != 0 {
                return Err(err);
            }
            let err = lfs_mkdir(lfs_ptr, path_d.as_ptr());
            if err != 0 {
                let _ = lfs_unmount(lfs_ptr);
                return Err(err);
            }
            let err = lfs_unmount(lfs_ptr);
            if err != 0 {
                return Err(err);
            }
            Ok(())
        },
        |lfs_ptr, config| {
            let err = lfs_mount(lfs_ptr, config);
            if err != 0 {
                return Err(err);
            }
            let _ = lfs_unmount(lfs_ptr);
            Ok(())
        },
    );
    result.expect("OOO powerloss linear should complete");
}

/// Minimal subdir: mkdir + file, single write + sync.
#[test]
fn test_debug_file_subdir_single_write_sync() {
    init_logger();
    let mut env = default_config(128);
    init_context(&mut env);

    let mut lfs = core::mem::MaybeUninit::<Lfs>::zeroed();
    assert_ok_at(
        "format",
        lfs_format(lfs.as_mut_ptr(), &env.config as *const LfsConfig),
    );
    assert_ok_at(
        "mount",
        lfs_mount(lfs.as_mut_ptr(), &env.config as *const LfsConfig),
    );

    let lfs_ptr = lfs.as_mut_ptr();
    assert_ok_at(
        "mkdir notebook",
        lfs_mkdir(lfs_ptr, path_bytes("notebook").as_ptr()),
    );

    let mut file = core::mem::MaybeUninit::<LfsFile>::zeroed();
    assert_ok_at(
        "file_open paper create",
        lfs_file_open(
            lfs_ptr,
            file.as_mut_ptr(),
            path_bytes("notebook/paper").as_ptr(),
            LFS_O_WRONLY | LFS_O_CREAT | LFS_O_APPEND,
        ),
    );
    let buf = b"hello";
    let n = lfs_file_write(
        lfs_ptr,
        file.as_mut_ptr(),
        buf.as_ptr() as *const core::ffi::c_void,
        buf.len() as u32,
    );
    assert_eq!(n, buf.len() as i32);
    assert_ok_at("file_sync", lfs_file_sync(lfs_ptr, file.as_mut_ptr()));
    assert_ok_at("file_close", lfs_file_close(lfs_ptr, file.as_mut_ptr()));
    assert_ok_at("unmount", lfs_unmount(lfs_ptr));
}

/// Open `path` with WRONLY|APPEND|`extra_flags`, write `data` `times` times
/// with a sync after each write, then close.
fn append_synced(lfs: *mut Lfs, path: &[u8], extra_flags: i32, data: &[u8], times: usize) {
    let mut file = core::mem::MaybeUninit::<LfsFile>::zeroed();
    assert_ok_at(
        "file_open append",
        lfs_file_open(
            lfs,
            file.as_mut_ptr(),
            path.as_ptr(),
            LFS_O_WRONLY | LFS_O_APPEND | extra_flags,
        ),
    );
    for i in 0..times {
        let n = lfs_file_write(
            lfs,
            file.as_mut_ptr(),
            data.as_ptr() as *const core::ffi::c_void,
            data.len() as u32,
        );
        assert_eq!(n, data.len() as i32, "file_write #{i}");
        assert_ok_at(
            &format!("file_sync #{i}"),
            lfs_file_sync(lfs, file.as_mut_ptr()),
        );
    }
    assert_ok_at("file_close", lfs_file_close(lfs, file.as_mut_ptr()));
}

/// Open `path` read-only and expect each `(chunk, times)` in order.
fn read_expect(lfs: *mut Lfs, path: &[u8], expected: &[(&[u8], usize)]) {
    let mut file = core::mem::MaybeUninit::<LfsFile>::zeroed();
    assert_ok_at(
        "file_open read",
        lfs_file_open(lfs, file.as_mut_ptr(), path.as_ptr(), LFS_O_RDONLY),
    );
    let mut rbuf = [0u8; 256];
    for &(chunk, times) in expected {
        for i in 0..times {
            let n = lfs_file_read(
                lfs,
                file.as_mut_ptr(),
                rbuf.as_mut_ptr() as *mut core::ffi::c_void,
                chunk.len() as u32,
            );
            assert_eq!(n, chunk.len() as i32, "file_read #{i}");
            assert_eq!(&rbuf[..chunk.len()], chunk, "file_read #{i}");
        }
    }
    assert_ok_at("file_close", lfs_file_close(lfs, file.as_mut_ptr()));
}
