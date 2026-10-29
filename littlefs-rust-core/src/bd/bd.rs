//! Block device operations. Per lfs.c lfs_bd_read, lfs_bd_prog, lfs_bd_crc, etc.

use crate::bd::LfsCache;
use crate::error::LFS_ERR_CORRUPT;
use crate::fs::Lfs;
use crate::types::{lfs_block_t, lfs_off_t, lfs_size_t};
use crate::util::{lfs_aligndown, lfs_alignup, lfs_min};

/// Per lfs.c lfs_cache_drop (lines 31-36)
///
/// C:
/// ```c
/// static inline void lfs_cache_drop(lfs_t *lfs, lfs_cache_t *rcache) {
///     // do not zero, cheaper if cache is readonly or only going to be
///     // written with identical data (during relocates)
///     (void)lfs;
///     rcache->block = LFS_BLOCK_NULL;
/// }
/// ```
#[inline(always)]
pub fn lfs_cache_drop(_lfs: *const Lfs, rcache: *mut LfsCache) {
    unsafe {
        (*rcache).block = crate::types::LFS_BLOCK_NULL;
    }
}

/// Per lfs.c lfs_cache_zero (lines 38-42)
///
/// C:
/// ```c
/// static inline void lfs_cache_zero(lfs_t *lfs, lfs_cache_t *pcache) {
///     // zero to avoid information leak
///     memset(pcache->buffer, 0xff, lfs->cfg->cache_size);
///     pcache->block = LFS_BLOCK_NULL;
/// }
/// ```
#[inline(always)]
pub fn lfs_cache_zero(lfs: *const Lfs, pcache: *mut LfsCache) {
    unsafe {
        let cfg = (*lfs).cfg;
        let cache_size = (*cfg).cache_size as usize;
        let buf = (*pcache).buffer;
        if !buf.is_null() {
            core::ptr::write_bytes(buf, 0xff, cache_size);
        }
        (*pcache).block = crate::types::LFS_BLOCK_NULL;
    }
}

/// Per lfs.c lfs_bd_read (lines 44-126)
///
/// C:
/// ```c
/// static int lfs_bd_read(lfs_t *lfs,
///         const lfs_cache_t *pcache, lfs_cache_t *rcache, lfs_size_t hint,
///         lfs_block_t block, lfs_off_t off,
///         void *buffer, lfs_size_t size) {
///     uint8_t *data = buffer;
///     if (off+size > lfs->cfg->block_size
///             || (lfs->block_count && block >= lfs->block_count)) {
///         return LFS_ERR_CORRUPT;
///     }
///
///     while (size > 0) {
///         lfs_size_t diff = size;
///
///         if (pcache && block == pcache->block &&
///                 off < pcache->off + pcache->size) {
///             if (off >= pcache->off) {
///                 // is already in pcache?
///                 diff = lfs_min(diff, pcache->size - (off-pcache->off));
///                 memcpy(data, &pcache->buffer[off-pcache->off], diff);
///
///                 data += diff;
///                 off += diff;
///                 size -= diff;
///                 continue;
///             }
///
///             // pcache takes priority
///             diff = lfs_min(diff, pcache->off-off);
///         }
///
///         if (block == rcache->block &&
///                 off < rcache->off + rcache->size) {
///             if (off >= rcache->off) {
///                 // is already in rcache?
///                 diff = lfs_min(diff, rcache->size - (off-rcache->off));
///                 memcpy(data, &rcache->buffer[off-rcache->off], diff);
///
///                 data += diff;
///                 off += diff;
///                 size -= diff;
///                 continue;
///             }
///
///             // rcache takes priority
///             diff = lfs_min(diff, rcache->off-off);
///         }
///
///         if (size >= hint && off % lfs->cfg->read_size == 0 &&
///                 size >= lfs->cfg->read_size) {
///             // bypass cache?
///             diff = lfs_aligndown(diff, lfs->cfg->read_size);
///             int err = lfs->cfg->read(lfs->cfg, block, off, data, diff);
///             LFS_ASSERT(err <= 0);
///             if (err) {
///                 return err;
///             }
///
///             data += diff;
///             off += diff;
///             size -= diff;
///             continue;
///         }
///
///         // load to cache, first condition can no longer fail
///         LFS_ASSERT(!lfs->block_count || block < lfs->block_count);
///         rcache->block = block;
///         rcache->off = lfs_aligndown(off, lfs->cfg->read_size);
///         rcache->size = lfs_min(
///                 lfs_min(
///                     lfs_alignup(off+hint, lfs->cfg->read_size),
///                     lfs->cfg->block_size)
///                 - rcache->off,
///                 lfs->cfg->cache_size);
///         int err = lfs->cfg->read(lfs->cfg, rcache->block,
///                 rcache->off, rcache->buffer, rcache->size);
///         LFS_ASSERT(err <= 0);
///         if (err) {
///             return err;
///         }
///     }
///
///     return 0;
/// }
/// ```
pub fn lfs_bd_read(
    lfs: *mut Lfs,
    pcache: *const LfsCache,
    rcache: *mut LfsCache,
    hint: lfs_size_t,
    block: lfs_block_t,
    off: lfs_off_t,
    buffer: *mut u8,
    size: lfs_size_t,
) -> i32 {
    unsafe {
        // The caches may be fields of `lfs`. Borrow only the disjoint
        // configuration/count, not the whole filesystem while using them.
        let cfg = &*(*lfs).cfg;
        let block_count = (*lfs).block_count;
        let read = match cfg.read {
            Some(f) => f,
            None => return LFS_ERR_CORRUPT,
        };

        if off + size > cfg.block_size || (block_count != 0 && block >= block_count) {
            return crate::lfs_err!(LFS_ERR_CORRUPT);
        }

        let mut data = buffer;
        let mut off = off;
        let mut size = size;

        while size > 0 {
            let mut diff = size;

            if !pcache.is_null() {
                let pcache = &*pcache;
                if block == pcache.block && off < pcache.off + pcache.size {
                    if off >= pcache.off {
                        diff = lfs_min(diff, pcache.size - (off - pcache.off));
                        if !pcache.buffer.is_null() {
                            core::ptr::copy_nonoverlapping(
                                pcache.buffer.add((off - pcache.off) as usize),
                                data,
                                diff as usize,
                            );
                        }
                        data = data.add(diff as usize);
                        off += diff;
                        size -= diff;
                        continue;
                    }
                    diff = lfs_min(diff, pcache.off - off);
                }
            }

            let rcache = &mut *rcache;
            if block == rcache.block && off < rcache.off + rcache.size {
                if off >= rcache.off {
                    diff = lfs_min(diff, rcache.size - (off - rcache.off));
                    if !rcache.buffer.is_null() {
                        core::ptr::copy_nonoverlapping(
                            rcache.buffer.add((off - rcache.off) as usize),
                            data,
                            diff as usize,
                        );
                    }
                    data = data.add(diff as usize);
                    off += diff;
                    size -= diff;
                    continue;
                }
                diff = lfs_min(diff, rcache.off - off);
            }

            if size >= hint && off.is_multiple_of(cfg.read_size) && size >= cfg.read_size {
                diff = lfs_aligndown(diff, cfg.read_size);
                crate::lfs_trace!("bd_read block={} off={} size={}", block, off, diff);
                let err = read(cfg as *const _, block, off, data, diff);
                crate::lfs_assert!(err <= 0);
                if err != 0 {
                    crate::lfs_trace!("bd_read block={} -> CORRUPT", block);
                    return crate::lfs_pass_err!(err);
                }
                data = data.add(diff as usize);
                off += diff;
                size -= diff;
                continue;
            }

            crate::lfs_assert!(block_count == 0 || block < block_count);
            rcache.block = block;
            rcache.off = lfs_aligndown(off, cfg.read_size);
            rcache.size = lfs_min(
                lfs_min(lfs_alignup(off + hint, cfg.read_size), cfg.block_size)
                    .saturating_sub(rcache.off),
                cfg.cache_size,
            );
            crate::lfs_trace!(
                "bd_read block={} off={} size={}",
                rcache.block,
                rcache.off,
                rcache.size
            );
            let err = read(
                cfg as *const _,
                rcache.block,
                rcache.off,
                rcache.buffer,
                rcache.size,
            );
            crate::lfs_assert!(err <= 0);
            if err != 0 {
                crate::lfs_trace!("bd_read block={} -> CORRUPT", rcache.block);
                // Don't leave rcache claiming to have this block when the buffer wasn't filled.
                // A retry (e.g. after bad-block clear) would otherwise serve stale data.
                rcache.block = crate::types::LFS_BLOCK_NULL;
                return crate::lfs_pass_err!(err);
            }
        }

        0
    }
}

/// Per lfs.c lfs_bd_cmp (lines 128-154)
///
/// C:
/// ```c
/// static int lfs_bd_cmp(lfs_t *lfs,
///         const lfs_cache_t *pcache, lfs_cache_t *rcache, lfs_size_t hint,
///         lfs_block_t block, lfs_off_t off,
///         const void *buffer, lfs_size_t size) {
///     const uint8_t *data = buffer;
///     lfs_size_t diff = 0;
///
///     for (lfs_off_t i = 0; i < size; i += diff) {
///         uint8_t dat[8];
///
///         diff = lfs_min(size-i, sizeof(dat));
///         int err = lfs_bd_read(lfs,
///                 pcache, rcache, hint-i,
///                 block, off+i, &dat, diff);
///         if (err) {
///             return err;
///         }
///
///         int res = memcmp(dat, data + i, diff);
///         if (res) {
///             return res < 0 ? LFS_CMP_LT : LFS_CMP_GT;
///         }
///     }
///
///     return LFS_CMP_EQ;
/// }
/// ```
pub fn lfs_bd_cmp(
    lfs: *mut Lfs,
    pcache: *const LfsCache,
    rcache: *mut LfsCache,
    hint: lfs_size_t,
    block: lfs_block_t,
    off: lfs_off_t,
    buffer: *const u8,
    size: lfs_size_t,
) -> i32 {
    // Per lfs.c enum: LFS_CMP_EQ=0, LFS_CMP_LT=1, LFS_CMP_GT=2 (positive = not error)
    const LFS_CMP_EQ: i32 = 0;
    const LFS_CMP_LT: i32 = 1;
    const LFS_CMP_GT: i32 = 2;

    let mut i: lfs_off_t = 0;
    while i < size {
        let mut dat = [0u8; 8];
        let diff = lfs_min(size - i, 8) as usize;
        let err = lfs_bd_read(
            lfs,
            pcache,
            rcache,
            hint.saturating_sub(i),
            block,
            off + i,
            dat.as_mut_ptr(),
            diff as lfs_size_t,
        );
        if err != 0 {
            return crate::lfs_pass_err!(err);
        }
        let res = unsafe {
            let disk = &dat[..diff];
            let expected = core::slice::from_raw_parts(buffer.add(i as usize), diff);
            disk.cmp(expected)
        };
        match res {
            core::cmp::Ordering::Equal => {}
            core::cmp::Ordering::Less => return LFS_CMP_LT,
            core::cmp::Ordering::Greater => return LFS_CMP_GT,
        }
        i += diff as lfs_off_t;
    }
    LFS_CMP_EQ
}

/// Per lfs.c lfs_bd_crc (lines 155-175)
///
/// C:
/// ```c
/// static int lfs_bd_crc(lfs_t *lfs,
///         const lfs_cache_t *pcache, lfs_cache_t *rcache, lfs_size_t hint,
///         lfs_block_t block, lfs_off_t off, lfs_size_t size, uint32_t *crc) {
///     lfs_size_t diff = 0;
///
///     for (lfs_off_t i = 0; i < size; i += diff) {
///         uint8_t dat[8];
///         diff = lfs_min(size-i, sizeof(dat));
///         int err = lfs_bd_read(lfs,
///                 pcache, rcache, hint-i,
///                 block, off+i, &dat, diff);
///         if (err) {
///             return err;
///         }
///
///         *crc = lfs_crc(*crc, &dat, diff);
///     }
///
///     return 0;
/// }
/// ```
pub fn lfs_bd_crc(
    lfs: *mut Lfs,
    pcache: *const LfsCache,
    rcache: *mut LfsCache,
    hint: lfs_size_t,
    block: lfs_block_t,
    off: lfs_off_t,
    size: lfs_size_t,
    crc: *mut u32,
) -> i32 {
    use crate::crc::lfs_crc;
    use crate::util::lfs_min;

    let mut i: lfs_off_t = 0;
    while i < size {
        let mut dat = [0u8; 8];
        let diff = lfs_min(size - i, 8) as usize;
        let err = lfs_bd_read(
            lfs,
            pcache,
            rcache,
            hint.saturating_sub(i),
            block,
            off + i,
            dat.as_mut_ptr(),
            diff as lfs_size_t,
        );
        if err != 0 {
            return crate::lfs_pass_err!(err);
        }
        unsafe {
            *crc = lfs_crc(*crc, dat.as_ptr(), diff);
        }
        i += diff as lfs_off_t;
    }
    0
}

/// Per lfs.c lfs_bd_flush (lines 177-210)
///
/// C:
/// ```c
/// #ifndef LFS_READONLY
/// static int lfs_bd_flush(lfs_t *lfs,
///         lfs_cache_t *pcache, lfs_cache_t *rcache, bool validate) {
///     if (pcache->block != LFS_BLOCK_NULL && pcache->block != LFS_BLOCK_INLINE) {
///         LFS_ASSERT(pcache->block < lfs->block_count);
///         lfs_size_t diff = lfs_alignup(pcache->size, lfs->cfg->prog_size);
///         int err = lfs->cfg->prog(lfs->cfg, pcache->block,
///                 pcache->off, pcache->buffer, diff);
///         LFS_ASSERT(err <= 0);
///         if (err) {
///             return err;
///         }
///
///         if (validate) {
///             // check data on disk
///             lfs_cache_drop(lfs, rcache);
///             int res = lfs_bd_cmp(lfs,
///                     NULL, rcache, diff,
///                     pcache->block, pcache->off, pcache->buffer, diff);
///             if (res < 0) {
///                 return res;
///             }
///
///             if (res != LFS_CMP_EQ) {
///                 return LFS_ERR_CORRUPT;
///             }
///         }
///
///         lfs_cache_zero(lfs, pcache);
///     }
///
///     return 0;
/// }
/// #endif
/// ```
pub fn lfs_bd_flush(
    lfs: *const Lfs,
    pcache: *mut LfsCache,
    rcache: *mut LfsCache,
    validate: bool,
) -> i32 {
    use crate::types::LFS_BLOCK_INLINE;
    use crate::util::lfs_alignup;

    unsafe {
        let lfs_ptr = lfs;
        let cfg = &*(*lfs).cfg;
        let block_count = (*lfs).block_count;
        let pcache = &mut *pcache;

        if pcache.block != crate::types::LFS_BLOCK_NULL && pcache.block != LFS_BLOCK_INLINE {
            crate::lfs_assert!(pcache.block < block_count);
            let diff = lfs_alignup(pcache.size, cfg.prog_size);
            crate::lfs_trace!(
                "bd_prog block={} off={} size={}",
                pcache.block,
                pcache.off,
                diff
            );
            let prog = match cfg.prog {
                Some(f) => f,
                None => return LFS_ERR_CORRUPT,
            };
            let err = prog(
                cfg as *const _,
                pcache.block,
                pcache.off,
                pcache.buffer,
                diff,
            );
            crate::lfs_assert!(err <= 0);
            if err != 0 {
                crate::lfs_trace!("bd_prog block={} -> CORRUPT", pcache.block);
                return crate::lfs_pass_err!(err);
            }

            if validate {
                lfs_cache_drop(lfs, rcache);
                let res = lfs_bd_cmp(
                    lfs_ptr as *mut Lfs,
                    core::ptr::null(),
                    rcache,
                    diff,
                    pcache.block,
                    pcache.off,
                    pcache.buffer,
                    diff,
                );
                if res < 0 {
                    return res;
                }
                if res != 0 {
                    return crate::lfs_err!(LFS_ERR_CORRUPT);
                }
            }

            lfs_cache_zero(lfs, pcache);
        }

        0
    }
}

/// Per lfs.c lfs_bd_sync (lines 213-226)
///
/// C:
/// ```c
/// #ifndef LFS_READONLY
/// static int lfs_bd_sync(lfs_t *lfs,
///         lfs_cache_t *pcache, lfs_cache_t *rcache, bool validate) {
///     lfs_cache_drop(lfs, rcache);
///
///     int err = lfs_bd_flush(lfs, pcache, rcache, validate);
///     if (err) {
///         return err;
///     }
///
///     err = lfs->cfg->sync(lfs->cfg);
///     LFS_ASSERT(err <= 0);
///     return err;
/// }
/// #endif
/// ```
pub fn lfs_bd_sync(
    lfs: *const Lfs,
    pcache: *mut LfsCache,
    rcache: *mut LfsCache,
    validate: bool,
) -> i32 {
    unsafe {
        lfs_cache_drop(lfs, rcache);

        let err = lfs_bd_flush(lfs, pcache, rcache, validate);
        if err != 0 {
            return crate::lfs_pass_err!(err);
        }

        let cfg = &*(*lfs).cfg;
        let sync = match cfg.sync {
            Some(f) => f,
            None => return LFS_ERR_CORRUPT,
        };
        let err = sync(cfg as *const _);
        crate::lfs_assert!(err <= 0);
        err
    }
}

/// Per lfs.c lfs_bd_prog (lines 228-274)
///
/// C:
/// ```c
/// #ifndef LFS_READONLY
/// static int lfs_bd_prog(lfs_t *lfs,
///         lfs_cache_t *pcache, lfs_cache_t *rcache, bool validate,
///         lfs_block_t block, lfs_off_t off,
///         const void *buffer, lfs_size_t size) {
///     const uint8_t *data = buffer;
///     LFS_ASSERT(block == LFS_BLOCK_INLINE || block < lfs->block_count);
///     LFS_ASSERT(off + size <= lfs->cfg->block_size);
///
///     while (size > 0) {
///         if (block == pcache->block &&
///                 off >= pcache->off &&
///                 off < pcache->off + lfs->cfg->cache_size) {
///             // already fits in pcache?
///             lfs_size_t diff = lfs_min(size,
///                     lfs->cfg->cache_size - (off-pcache->off));
///             memcpy(&pcache->buffer[off-pcache->off], data, diff);
///
///             data += diff;
///             off += diff;
///             size -= diff;
///
///             pcache->size = lfs_max(pcache->size, off - pcache->off);
///             if (pcache->size == lfs->cfg->cache_size) {
///                 // eagerly flush out pcache if we fill up
///                 int err = lfs_bd_flush(lfs, pcache, rcache, validate);
///                 if (err) {
///                     return err;
///                 }
///             }
///
///             continue;
///         }
///
///         // pcache must have been flushed, either by programming and
///         // entire block or manually flushing the pcache
///         LFS_ASSERT(pcache->block == LFS_BLOCK_NULL);
///
///         // prepare pcache, first condition can no longer fail
///         pcache->block = block;
///         pcache->off = lfs_aligndown(off, lfs->cfg->prog_size);
///         pcache->size = 0;
///     }
///
///     return 0;
/// }
/// #endif
/// ```
pub fn lfs_bd_prog(
    lfs: *const Lfs,
    pcache: *mut LfsCache,
    rcache: *mut LfsCache,
    validate: bool,
    block: lfs_block_t,
    off: lfs_off_t,
    buffer: *const u8,
    size: lfs_size_t,
) -> i32 {
    use crate::types::LFS_BLOCK_INLINE;
    use crate::util::{lfs_aligndown, lfs_max, lfs_min};

    unsafe {
        let cfg = &*(*lfs).cfg;
        let block_count = (*lfs).block_count;
        let pcache = &mut *pcache;

        crate::lfs_assert!(block == LFS_BLOCK_INLINE || block < block_count);
        crate::lfs_assert!(off + size <= cfg.block_size);

        let mut data = buffer;
        let mut off = off;
        let mut size = size;

        while size > 0 {
            if block == pcache.block && off >= pcache.off && off < pcache.off + cfg.cache_size {
                let diff = lfs_min(size, cfg.cache_size - (off - pcache.off));
                if !pcache.buffer.is_null() && !data.is_null() {
                    // Trace superblock magic region (offset 12-20 in block 0/1)
                    if (block == 0 || block == 1) && off <= 12 && off + diff > 12 {
                        let magic_start = 12usize.saturating_sub(off as usize);
                        let magic_len = (8).min(diff as usize - magic_start);
                        if magic_len > 0 {
                            let slice =
                                core::slice::from_raw_parts(data.add(magic_start), magic_len);
                            crate::lfs_trace!(
                                "bd_prog superblock block={} off={} size={} magic_region[{}..{}]={:?}",
                                block,
                                off,
                                size,
                                magic_start,
                                magic_start + magic_len,
                                slice
                            );
                        }
                    }
                    core::ptr::copy_nonoverlapping(
                        data,
                        pcache.buffer.add((off - pcache.off) as usize),
                        diff as usize,
                    );
                }

                data = data.add(diff as usize);
                off += diff;
                size -= diff;

                pcache.size = lfs_max(pcache.size, off - pcache.off);
                if pcache.size == cfg.cache_size {
                    let err = lfs_bd_flush(lfs, pcache, rcache, validate);
                    if err != 0 {
                        return crate::lfs_pass_err!(err);
                    }
                }

                continue;
            }

            crate::lfs_assert!(pcache.block == crate::types::LFS_BLOCK_NULL);

            pcache.block = block;
            pcache.off = lfs_aligndown(off, cfg.prog_size);
            pcache.size = 0;
        }

        0
    }
}

/// Per lfs.c lfs_bd_erase (lines 277-282)
///
/// C:
/// ```c
/// #ifndef LFS_READONLY
/// static int lfs_bd_erase(lfs_t *lfs, lfs_block_t block) {
///     LFS_ASSERT(block < lfs->block_count);
///     int err = lfs->cfg->erase(lfs->cfg, block);
///     LFS_ASSERT(err <= 0);
///     return err;
/// }
/// #endif
/// ```
pub fn lfs_bd_erase(lfs: *const Lfs, block: lfs_block_t) -> i32 {
    unsafe {
        let lfs = &*lfs;
        crate::lfs_assert!(block < lfs.block_count);
        let erase = match (*lfs.cfg).erase {
            Some(f) => f,
            None => return LFS_ERR_CORRUPT,
        };
        crate::lfs_trace!("bd_erase block={}", block);
        let err = erase(lfs.cfg, block);
        crate::lfs_assert!(err <= 0);
        if err != 0 {
            crate::lfs_trace!("bd_erase block={} -> CORRUPT", block);
        }
        err
    }
}
