//! Directory commit. Per lfs.c lfs_dir_commit, lfs_dir_commitattr, lfs_dir_alloc, etc.

use crate::dir::fetch::lfs_dir_getgstate;
use crate::dir::LfsCommit;
use crate::dir::LfsMdir;
use crate::fs::stat::lfs_fs_size_;
use crate::fs::Lfs;
use crate::types::{lfs_block_t, lfs_off_t, lfs_size_t, lfs_tag_t};

/// Per lfs.c lfs_dir_commitprog (lines 1604-1618)
///
/// C:
/// ```c
/// static int lfs_dir_commitprog(lfs_t *lfs, struct lfs_commit *commit,
///         const void *buffer, lfs_size_t size) {
///     int err = lfs_bd_prog(lfs,
///             &lfs->pcache, &lfs->rcache, false,
///             commit->block, commit->off ,
///             (const uint8_t*)buffer, size);
///     if (err) {
///         return err;
///     }
///
///     commit->crc = lfs_crc(commit->crc, buffer, size);
///     commit->off += size;
///     return 0;
/// }
/// ```
pub fn lfs_dir_commitprog(
    lfs: *mut crate::fs::Lfs,
    commit: *mut LfsCommit,
    buffer: *const core::ffi::c_void,
    size: lfs_size_t,
) -> i32 {
    use crate::bd::bd::lfs_bd_prog;
    use crate::crc::lfs_crc;

    unsafe {
        let lfs_ref = &mut *lfs;
        let commit_ref = &mut *commit;
        let buf = buffer as *const u8;

        let err = lfs_bd_prog(
            lfs_ref,
            &mut lfs_ref.pcache,
            &mut lfs_ref.rcache,
            false,
            commit_ref.block,
            commit_ref.off,
            buf,
            size,
        );
        if err != 0 {
            return crate::lfs_pass_err!(err);
        }

        commit_ref.crc = lfs_crc(commit_ref.crc, buf, size as usize);
        commit_ref.off += size;
        0
    }
}

/// Per lfs.c lfs_dir_commitattr (lines 1621-1666)
///
/// C:
/// ```c
/// static int lfs_dir_commitattr(lfs_t *lfs, struct lfs_commit *commit,
///         lfs_tag_t tag, const void *buffer) {
///     // check if we fit
///     lfs_size_t dsize = lfs_tag_dsize(tag);
///     if (commit->off + dsize > commit->end) {
///         return LFS_ERR_NOSPC;
///     }
///
///     // write out tag
///     lfs_tag_t ntag = lfs_tobe32((tag & 0x7fffffff) ^ commit->ptag);
///     int err = lfs_dir_commitprog(lfs, commit, &ntag, sizeof(ntag));
///     if (err) {
///         return err;
///     }
///
///     if (!(tag & 0x80000000)) {
///         // from memory
///         err = lfs_dir_commitprog(lfs, commit, buffer, dsize-sizeof(tag));
///         if (err) {
///             return err;
///         }
///     } else {
///         // from disk
///         const struct lfs_diskoff *disk = buffer;
///         for (lfs_off_t i = 0; i < dsize-sizeof(tag); i++) {
///             uint8_t dat;
///             err = lfs_bd_read(lfs, NULL, &lfs->rcache, dsize-sizeof(tag)-i,
///                     disk->block, disk->off+i, &dat, 1);
///             if (err) return err;
///             err = lfs_dir_commitprog(lfs, commit, &dat, 1);
///             if (err) return err;
///         }
///     }
///
///     commit->ptag = tag & 0x7fffffff;
///     return 0;
/// }
/// ```
pub fn lfs_dir_commitattr(
    lfs: *mut crate::fs::Lfs,
    commit: *mut LfsCommit,
    tag: lfs_tag_t,
    buffer: *const core::ffi::c_void,
) -> i32 {
    use crate::bd::bd::lfs_bd_read;
    use crate::error::LFS_ERR_NOSPC;
    use crate::tag::{lfs_tag_dsize, lfs_tag_isvalid};
    use crate::util::lfs_tobe32;

    unsafe {
        let commit_ref = &mut *commit;
        let dsize = lfs_tag_dsize(tag);

        if commit_ref.off + dsize > commit_ref.end {
            crate::lfs_trace!(
                "lfs_dir_commitattr NOSPC: off+dsize>end off={} dsize={} end={} block={}",
                commit_ref.off,
                dsize,
                commit_ref.end,
                commit_ref.block
            );
            return crate::lfs_err!(LFS_ERR_NOSPC);
        }

        let ntag = lfs_tobe32((tag & 0x7fff_ffff) ^ commit_ref.ptag);
        let mut err = lfs_dir_commitprog(lfs, commit, &ntag as *const _ as *const _, 4);
        if err != 0 {
            return crate::lfs_pass_err!(err);
        }

        if u32::from(crate::tag::lfs_tag_type1(tag))
            == crate::lfs_type::lfs_type::LFS_TYPE_SUPERBLOCK
        {
            crate::lfs_trace!(
                "commitattr SUPERBLOCK: dsize={} buffer={:p} commit.block={} commit.off={}",
                dsize,
                buffer,
                commit_ref.block,
                commit_ref.off
            );
            if !buffer.is_null() && dsize >= 8 {
                crate::lfs_trace!(
                    "commitattr SUPERBLOCK data (first 8): {:?}",
                    core::slice::from_raw_parts(buffer as *const u8, 8)
                );
            }
        }

        if lfs_tag_isvalid(tag) {
            err = lfs_dir_commitprog(lfs, commit, buffer, dsize.saturating_sub(4));
            if err != 0 {
                return crate::lfs_pass_err!(err);
            }
        } else {
            let disk = buffer as *const crate::tag::lfs_diskoff;
            let disk_ref = &*disk;
            let data_size = dsize.saturating_sub(4);
            for i in 0..data_size {
                let mut dat: u8 = 0;
                err = lfs_bd_read(
                    lfs,
                    core::ptr::null(),
                    &mut (*lfs).rcache,
                    data_size - i,
                    disk_ref.block,
                    disk_ref.off + i,
                    &mut dat as *mut u8,
                    1,
                );
                if err != 0 {
                    return crate::lfs_pass_err!(err);
                }
                err = lfs_dir_commitprog(lfs, commit, &dat as *const _ as *const _, 1);
                if err != 0 {
                    return crate::lfs_pass_err!(err);
                }
            }
        }

        commit_ref.ptag = tag & 0x7fff_ffff;
        0
    }
}

/// Per lfs.c lfs_dir_commitcrc (lines 1669-1812)
///
/// C:
/// ```c
/// static int lfs_dir_commitcrc(lfs_t *lfs, struct lfs_commit *commit) {
///     // align to program units
///     //
///     // this gets a bit complex as we have two types of crcs:
///     // - 5-word crc with fcrc to check following prog (middle of block)
///     // - 2-word crc with no following prog (end of block)
///     const lfs_off_t end = lfs_alignup(
///             lfs_min(commit->off + 5*sizeof(uint32_t), lfs->cfg->block_size),
///             lfs->cfg->prog_size);
///
///     lfs_off_t off1 = 0;
///     uint32_t crc1 = 0;
///
///     // create crc tags to fill up remainder of commit, note that
///     // padding is not crced, which lets fetches skip padding but
///     // makes committing a bit more complicated
///     while (commit->off < end) {
///         lfs_off_t noff = (
///                 lfs_min(end - (commit->off+sizeof(lfs_tag_t)), 0x3fe)
///                 + (commit->off+sizeof(lfs_tag_t)));
///         // too large for crc tag? need padding commits
///         if (noff < end) {
///             noff = lfs_min(noff, end - 5*sizeof(uint32_t));
///         }
///
///         // space for fcrc?
///         uint8_t eperturb = (uint8_t)-1;
///         if (noff >= end && noff <= lfs->cfg->block_size - lfs->cfg->prog_size) {
///             // first read the leading byte, this always contains a bit
///             // we can perturb to avoid writes that don't change the fcrc
///             int err = lfs_bd_read(lfs,
///                     NULL, &lfs->rcache, lfs->cfg->prog_size,
///                     commit->block, noff, &eperturb, 1);
///             if (err && err != LFS_ERR_CORRUPT) {
///                 return err;
///             }
///
///         #ifdef LFS_MULTIVERSION
///             // unfortunately fcrcs break mdir fetching < lfs2.1, so only write
///             // these if we're a >= lfs2.1 filesystem
///             if (lfs_fs_disk_version(lfs) <= 0x00020000) {
///                 // don't write fcrc
///             } else
///         #endif
///             {
///                 // find the expected fcrc, don't bother avoiding a reread
///                 // of the eperturb, it should still be in our cache
///                 struct lfs_fcrc fcrc = {
///                     .size = lfs->cfg->prog_size,
///                     .crc = 0xffffffff
///                 };
///                 err = lfs_bd_crc(lfs,
///                         NULL, &lfs->rcache, lfs->cfg->prog_size,
///                         commit->block, noff, fcrc.size, &fcrc.crc);
///                 if (err && err != LFS_ERR_CORRUPT) {
///                     return err;
///                 }
///
///                 lfs_fcrc_tole32(&fcrc);
///                 err = lfs_dir_commitattr(lfs, commit,
///                         LFS_MKTAG(LFS_TYPE_FCRC, 0x3ff, sizeof(struct lfs_fcrc)),
///                         &fcrc);
///                 if (err) {
///                     return err;
///                 }
///             }
///         }
///
///         // build commit crc
///         struct {
///             lfs_tag_t tag;
///             uint32_t crc;
///         } ccrc;
///         lfs_tag_t ntag = LFS_MKTAG(
///                 LFS_TYPE_CCRC + (((uint8_t)~eperturb) >> 7), 0x3ff,
///                 noff - (commit->off+sizeof(lfs_tag_t)));
///         ccrc.tag = lfs_tobe32(ntag ^ commit->ptag);
///         commit->crc = lfs_crc(commit->crc, &ccrc.tag, sizeof(lfs_tag_t));
///         ccrc.crc = lfs_tole32(commit->crc);
///
///         int err = lfs_bd_prog(lfs,
///                 &lfs->pcache, &lfs->rcache, false,
///                 commit->block, commit->off, &ccrc, sizeof(ccrc));
///         if (err) {
///             return err;
///         }
///
///         // keep track of non-padding checksum to verify
///         if (off1 == 0) {
///             off1 = commit->off + sizeof(lfs_tag_t);
///             crc1 = commit->crc;
///         }
///
///         commit->off = noff;
///         // perturb valid bit?
///         commit->ptag = ntag ^ ((0x80UL & ~eperturb) << 24);
///         // reset crc for next commit
///         commit->crc = 0xffffffff;
///
///         // manually flush here since we don't prog the padding, this confuses
///         // the caching layer
///         if (noff >= end || noff >= lfs->pcache.off + lfs->cfg->cache_size) {
///             // flush buffers
///             int err = lfs_bd_sync(lfs, &lfs->pcache, &lfs->rcache, false);
///             if (err) {
///                 return err;
///             }
///         }
///     }
///
///     // successful commit, check checksums to make sure
///     //
///     // note that we don't need to check padding commits, worst
///     // case if they are corrupted we would have had to compact anyways
///     lfs_off_t off = commit->begin;
///     uint32_t crc = 0xffffffff;
///     int err = lfs_bd_crc(lfs,
///             NULL, &lfs->rcache, off1+sizeof(uint32_t),
///             commit->block, off, off1-off, &crc);
///     if (err) {
///         return err;
///     }
///
///     // check non-padding commits against known crc
///     if (crc != crc1) {
///         return LFS_ERR_CORRUPT;
///     }
///
///     // make sure to check crc in case we happen to pick
///     // up an unrelated crc (frozen block?)
///     err = lfs_bd_crc(lfs,
///             NULL, &lfs->rcache, sizeof(uint32_t),
///             commit->block, off1, sizeof(uint32_t), &crc);
///     if (err) {
///         return err;
///     }
///
///     if (crc != 0) {
///         return LFS_ERR_CORRUPT;
///     }
///
///     return 0;
/// }
/// #endif
/// ```
pub fn lfs_dir_commitcrc(lfs: *mut crate::fs::Lfs, commit: *mut LfsCommit) -> i32 {
    use crate::bd::bd::{lfs_bd_crc, lfs_bd_prog, lfs_bd_sync};
    use crate::crc::lfs_crc;
    use crate::dir::lfs_fcrc::lfs_fcrc_tole32;
    use crate::dir::LfsFcrc;
    use crate::error::LFS_ERR_CORRUPT;
    use crate::tag::lfs_mktag;
    use crate::util::{lfs_alignup, lfs_min, lfs_tobe32, lfs_tole32};

    unsafe {
        let lfs_ref = &*lfs;
        let cfg = lfs_ref.cfg.as_ref().unwrap();
        let block_size = cfg.block_size;
        let prog_size = cfg.prog_size;

        let end = lfs_alignup(lfs_min((*commit).off + 20, block_size), prog_size);

        let mut off1: lfs_off_t = 0;
        let mut crc1: u32 = 0;

        while (*commit).off < end {
            let noff = lfs_min(end - ((*commit).off + 4), 0x3fe) + ((*commit).off + 4);
            let noff = if noff < end {
                lfs_min(noff, end - 20)
            } else {
                noff
            };

            let mut eperturb: u8 = 0xff;
            if noff >= end && noff <= block_size - prog_size {
                let err = crate::bd::bd::lfs_bd_read(
                    lfs,
                    core::ptr::null_mut(),
                    &mut (*lfs).rcache,
                    prog_size,
                    (*commit).block,
                    noff,
                    &mut eperturb,
                    1,
                );
                if err != 0 && err != crate::error::LFS_ERR_CORRUPT {
                    return crate::lfs_pass_err!(err);
                }

                // C: the LFS_MULTIVERSION branch that skips the fcrc on
                // lfs2.0 disks is not ported; lfs_dir_fetchmatch likewise
                // omits it, and this port always formats lfs2.1 disks.

                // find the expected fcrc, don't bother avoiding a reread
                // of the eperturb, it should still be in our cache
                let mut fcrc = LfsFcrc {
                    size: prog_size,
                    crc: 0xffff_ffff,
                };
                let err = lfs_bd_crc(
                    lfs,
                    core::ptr::null(),
                    &mut (*lfs).rcache,
                    prog_size,
                    (*commit).block,
                    noff,
                    fcrc.size,
                    &mut fcrc.crc,
                );
                if err != 0 && err != crate::error::LFS_ERR_CORRUPT {
                    return crate::lfs_pass_err!(err);
                }

                lfs_fcrc_tole32(&mut fcrc);
                let err = lfs_dir_commitattr(
                    lfs,
                    commit,
                    lfs_mktag(
                        crate::lfs_type::lfs_type::LFS_TYPE_FCRC,
                        0x3ff,
                        core::mem::size_of::<LfsFcrc>() as u32,
                    ),
                    &fcrc as *const LfsFcrc as *const core::ffi::c_void,
                );
                if err != 0 {
                    return crate::lfs_pass_err!(err);
                }
            }

            let ntag = lfs_mktag(
                crate::lfs_type::lfs_type::LFS_TYPE_CCRC + (u32::from(!eperturb) >> 7),
                0x3ff,
                noff - ((*commit).off + 4),
            );

            let xor_tag = lfs_tobe32(ntag ^ (*commit).ptag);
            (*commit).crc = lfs_crc((*commit).crc, &xor_tag as *const _ as *const u8, 4);
            let crc_le = lfs_tole32((*commit).crc);

            let mut ccrc: [u8; 8] = [0; 8];
            core::ptr::copy_nonoverlapping(&xor_tag as *const _ as *const u8, ccrc.as_mut_ptr(), 4);
            core::ptr::copy_nonoverlapping(
                &crc_le as *const _ as *const u8,
                ccrc.as_mut_ptr().add(4),
                4,
            );

            let err = lfs_bd_prog(
                lfs,
                &mut (*lfs).pcache,
                &mut (*lfs).rcache,
                false,
                (*commit).block,
                (*commit).off,
                ccrc.as_ptr(),
                8,
            );
            if err != 0 {
                return crate::lfs_pass_err!(err);
            }

            if off1 == 0 {
                off1 = (*commit).off + 4;
                crc1 = (*commit).crc;
            }

            (*commit).off = noff;
            (*commit).ptag = ntag ^ ((0x80 & !eperturb) as u32) << 24;
            (*commit).crc = 0xffff_ffff;

            if noff >= end || noff >= (*lfs).pcache.off + cfg.cache_size {
                let err = lfs_bd_sync(lfs, &mut (*lfs).pcache, &mut (*lfs).rcache, false);
                if err != 0 {
                    return crate::lfs_pass_err!(err);
                }
            }
        }

        let mut crc: u32 = 0xffff_ffff;
        let err = lfs_bd_crc(
            lfs,
            core::ptr::null(),
            &mut (*lfs).rcache,
            off1 + 4,
            (*commit).block,
            (*commit).begin,
            off1 - (*commit).begin,
            &mut crc,
        );
        if err != 0 {
            return crate::lfs_pass_err!(err);
        }
        if crc != crc1 {
            return crate::lfs_err!(LFS_ERR_CORRUPT);
        }

        let err = lfs_bd_crc(
            lfs,
            core::ptr::null(),
            &mut (*lfs).rcache,
            4,
            (*commit).block,
            off1,
            4,
            &mut crc,
        );
        if err != 0 {
            return crate::lfs_pass_err!(err);
        }
        if crc != 0 {
            return crate::lfs_err!(LFS_ERR_CORRUPT);
        }

        0
    }
}

/// Per lfs.c lfs_dir_alloc (lines 1815-1857)
///
/// C:
/// ```c
/// static int lfs_dir_alloc(lfs_t *lfs, lfs_mdir_t *dir) {
///     // allocate pair of dir blocks (backwards, so we write block 1 first)
///     for (int i = 0; i < 2; i++) {
///         int err = lfs_alloc(lfs, &dir->pair[(i+1)%2]);
///         if (err) {
///             return err;
///         }
///     }
///
///     // zero for reproducibility in case initial block is unreadable
///     dir->rev = 0;
///
///     // rather than clobbering one of the blocks we just pretend
///     // the revision may be valid
///     int err = lfs_bd_read(lfs,
///             NULL, &lfs->rcache, sizeof(dir->rev),
///             dir->pair[0], 0, &dir->rev, sizeof(dir->rev));
///     dir->rev = lfs_fromle32(dir->rev);
///     if (err && err != LFS_ERR_CORRUPT) {
///         return err;
///     }
///
///     // to make sure we don't immediately evict, align the new revision count
///     // to our block_cycles modulus, see lfs_dir_compact for why our modulus
///     // is tweaked this way
///     if (lfs->cfg->block_cycles > 0) {
///         dir->rev = lfs_alignup(dir->rev, ((lfs->cfg->block_cycles+1)|1));
///     }
///
///     // set defaults
///     dir->off = sizeof(dir->rev);
///     dir->etag = 0xffffffff;
///     dir->count = 0;
///     dir->tail[0] = LFS_BLOCK_NULL;
///     dir->tail[1] = LFS_BLOCK_NULL;
///     dir->erased = false;
///     dir->split = false;
///
///     // don't write out yet, let caller take care of that
///     return 0;
/// }
/// ```
///
/// # Safety
///
/// `lfs` and `dir` must be valid, properly initialized pointers.
pub unsafe fn lfs_dir_alloc(lfs: *mut crate::fs::Lfs, dir: *mut LfsMdir) -> i32 {
    use crate::bd::bd::lfs_bd_read;
    use crate::block_alloc::alloc::lfs_alloc;
    use crate::types::LFS_BLOCK_NULL;
    use crate::util::{lfs_alignup, lfs_fromle32};

    unsafe {
        let lfs_ref = &*lfs;
        let dir_ref = &mut *dir;

        for i in 0..2 {
            let out_block = &mut dir_ref.pair[(i + 1) % 2];
            let err = lfs_alloc(lfs, out_block);
            if err != 0 {
                return crate::lfs_pass_err!(err);
            }
        }

        dir_ref.rev = 0;

        let mut rev_buf: u32 = 0;
        let err = lfs_bd_read(
            lfs,
            core::ptr::null(),
            &mut (*lfs).rcache,
            core::mem::size_of::<u32>() as u32,
            dir_ref.pair[0],
            0,
            &mut rev_buf as *mut u32 as *mut u8,
            core::mem::size_of::<u32>() as u32,
        );
        dir_ref.rev = lfs_fromle32(rev_buf);
        if err != 0 && err != crate::error::LFS_ERR_CORRUPT {
            return crate::lfs_pass_err!(err);
        }

        if lfs_ref.cfg.as_ref().is_some_and(|c| c.block_cycles > 0) {
            let modulus = (lfs_ref.cfg.as_ref().unwrap().block_cycles as u32 + 1) | 1;
            dir_ref.rev = lfs_alignup(dir_ref.rev, modulus);
        }

        dir_ref.off = core::mem::size_of::<u32>() as u32;
        dir_ref.etag = 0xffff_ffff;
        dir_ref.count = 0;
        dir_ref.tail = [LFS_BLOCK_NULL, LFS_BLOCK_NULL];
        dir_ref.erased = false;
        dir_ref.split = false;

        0
    }
}

/// Per lfs.c lfs_dir_drop (lines 1859-1878)
///
/// C:
/// ```c
/// static int lfs_dir_drop(lfs_t *lfs, lfs_mdir_t *dir, lfs_mdir_t *tail) {
///     // steal state
///     int err = lfs_dir_getgstate(lfs, tail, &lfs->gdelta);
///     if (err) {
///         return err;
///     }
///
///     // steal tail
///     lfs_pair_tole32(tail->tail);
///     err = lfs_dir_commit(lfs, dir, LFS_MKATTRS(
///             {LFS_MKTAG(LFS_TYPE_TAIL + tail->split, 0x3ff, 8), tail->tail}));
///     lfs_pair_fromle32(tail->tail);
///     if (err) {
///         return err;
///     }
///
///     return 0;
/// }
/// ```
pub fn lfs_dir_drop(lfs: *mut crate::fs::Lfs, dir: *mut LfsMdir, tail: *const LfsMdir) -> i32 {
    use crate::lfs_type::lfs_type::LFS_TYPE_TAIL;
    use crate::tag::lfs_mktag;
    use crate::util::lfs_pair_tole32;

    unsafe {
        let err = lfs_dir_getgstate(lfs, tail, &mut (*lfs).gdelta);
        if err != 0 {
            return crate::lfs_pass_err!(err);
        }

        let tail_ref = &*tail;
        let mut tail_pair = tail_ref.tail;
        lfs_pair_tole32(&mut tail_pair);
        let attrs = [crate::tag::lfs_mattr {
            tag: lfs_mktag(LFS_TYPE_TAIL + if tail_ref.split { 1 } else { 0 }, 0x3ff, 8),
            buffer: tail_pair.as_ptr() as *const core::ffi::c_void,
        }];
        lfs_dir_commit(lfs, dir, attrs.as_ptr() as *const _, 1)
    }
}

/// Per lfs.c lfs_dir_split (lines 1880-1913)
///
/// Translation docs: Splits a directory by allocating a new tail metadata pair,
/// compacting entries [split, end) into it, and updating dir to point to the new tail.
/// When splitting the root (split==0), updates lfs->root to the new tail.
///
/// C:
/// ```c
/// static int lfs_dir_split(lfs_t *lfs,
///         lfs_mdir_t *dir, const struct lfs_mattr *attrs, int attrcount,
///         lfs_mdir_t *source, uint16_t split, uint16_t end) {
///     // create tail metadata pair
///     lfs_mdir_t tail;
///     int err = lfs_dir_alloc(lfs, &tail);
///     if (err) {
///         return err;
///     }
///
///     tail.split = dir->split;
///     tail.tail[0] = dir->tail[0];
///     tail.tail[1] = dir->tail[1];
///
///     // note we don't care about LFS_OK_RELOCATED
///     int res = lfs_dir_compact(lfs, &tail, attrs, attrcount, source, split, end);
///     if (res < 0) {
///         return res;
///     }
///
///     dir->tail[0] = tail.pair[0];
///     dir->tail[1] = tail.pair[1];
///     dir->split = true;
///
///     // update root if needed
///     if (lfs_pair_cmp(dir->pair, lfs->root) == 0 && split == 0) {
///         lfs->root[0] = tail.pair[0];
///         lfs->root[1] = tail.pair[1];
///     }
///
///     return 0;
/// }
/// ```
pub fn lfs_dir_split(
    lfs: *mut Lfs,
    dir: *mut LfsMdir,
    attrs: *const core::ffi::c_void,
    attrcount: i32,
    source: *const LfsMdir,
    split: u16,
    end: u16,
) -> i32 {
    use crate::util::lfs_pair_cmp;

    unsafe {
        let mut tail = LfsMdir {
            pair: [0, 0],
            rev: 0,
            off: 0,
            etag: 0,
            count: 0,
            erased: false,
            split: false,
            tail: [0, 0],
        };

        let err = lfs_dir_alloc(lfs, &mut tail);
        if err != 0 {
            return crate::lfs_pass_err!(err);
        }

        let dir_ref = &mut *dir;
        tail.split = dir_ref.split;
        tail.tail[0] = dir_ref.tail[0];
        tail.tail[1] = dir_ref.tail[1];

        // note we don't care about LFS_OK_RELOCATED
        let res = lfs_dir_compact(lfs, &mut tail, attrs, attrcount, source, split, end);
        if res < 0 {
            return res;
        }

        dir_ref.tail[0] = tail.pair[0];
        dir_ref.tail[1] = tail.pair[1];
        dir_ref.split = true;

        crate::lfs_trace!(
            "dir_split: split={} end={} new_tail=[{},{}] dir.pair=[{},{}] dir.tail=[{},{}]",
            split,
            end,
            tail.pair[0],
            tail.pair[1],
            dir_ref.pair[0],
            dir_ref.pair[1],
            dir_ref.tail[0],
            dir_ref.tail[1]
        );

        // update root if needed
        let root = &(*lfs).root;
        if lfs_pair_cmp(&dir_ref.pair, root) == 0 && split == 0 {
            (*lfs).root[0] = tail.pair[0];
            (*lfs).root[1] = tail.pair[1];
        }

        0
    }
}

/// Per lfs.c lfs_dir_commit_size (lines 1915-1923)
///
/// C:
/// ```c
/// static int lfs_dir_commit_size(void *p, lfs_tag_t tag, const void *buffer) {
///     lfs_size_t *size = p;
///     (void)buffer;
///
///     *size += lfs_tag_dsize(tag);
///     return 0;
/// }
/// ```
pub fn lfs_dir_commit_size(
    p: *mut core::ffi::c_void,
    tag: lfs_tag_t,
    _buffer: *const core::ffi::c_void,
) -> i32 {
    use crate::tag::lfs_tag_dsize;
    use crate::types::lfs_size_t;
    unsafe {
        let size = p as *mut lfs_size_t;
        *size += lfs_tag_dsize(tag);
    }
    0
}

/// Per lfs.c lfs_dir_commit_commit (lines 1932-1936)
///
/// C:
/// ```c
/// static int lfs_dir_commit_commit(void *p, lfs_tag_t tag, const void *buffer) {
///     struct lfs_dir_commit_commit *commit = p;
///     return lfs_dir_commitattr(commit->lfs, commit->commit, tag, buffer);
/// }
/// ```
pub fn lfs_dir_commit_commit(
    p: *mut core::ffi::c_void,
    tag: lfs_tag_t,
    buffer: *const core::ffi::c_void,
) -> i32 {
    if p.is_null() {
        return crate::error::LFS_ERR_INVAL;
    }
    unsafe {
        let commit_commit = &*(p as *const (*mut Lfs, *mut LfsCommit));
        let (lfs, commit) = *commit_commit;
        lfs_dir_commitattr(lfs, commit, tag, buffer)
    }
}

/// Per lfs.c lfs_dir_needsrelocation (lines 1939-1949)
///
/// C:
/// ```c
/// static bool lfs_dir_needsrelocation(lfs_t *lfs, lfs_mdir_t *dir) {
///     // If our revision count == n * block_cycles, we should force a relocation,
///     // this is how littlefs wear-levels at the metadata-pair level. Note that we
///     // actually use (block_cycles+1)|1, this is to avoid two corner cases:
///     // 1. block_cycles = 1, which would prevent relocations from terminating
///     // 2. block_cycles = 2n, which, due to aliasing, would only ever relocate
///     //    one metadata block in the pair, effectively making this useless
///     return (lfs->cfg->block_cycles > 0
///             && ((dir->rev + 1) % ((lfs->cfg->block_cycles+1)|1) == 0));
/// }
/// ```
pub fn lfs_dir_needsrelocation(lfs: *const Lfs, dir: *const LfsMdir) -> bool {
    unsafe {
        let cfg = (*lfs).cfg.as_ref();
        match cfg {
            None => false,
            Some(c) if c.block_cycles <= 0 => false, // C: block_cycles > 0 required
            Some(c) => {
                let modulus = ((c.block_cycles as u32).wrapping_add(1)) | 1;
                let dir_ref = &*dir;
                (dir_ref.rev.wrapping_add(1)).is_multiple_of(modulus)
            }
        }
    }
}

/// Per lfs.c lfs_dir_compact (lines 1952-2123)
///
/// C:
/// ```c
/// static int lfs_dir_compact(lfs_t *lfs,
///         lfs_mdir_t *dir, const struct lfs_mattr *attrs, int attrcount,
///         lfs_mdir_t *source, uint16_t begin, uint16_t end) {
///     // save some state in case block is bad
///     bool relocated = false;
///     bool tired = lfs_dir_needsrelocation(lfs, dir);
///
///     // increment revision count
///     dir->rev += 1;
///
///     // do not proactively relocate blocks during migrations, this
///     // can cause a number of failure states such: clobbering the
///     // v1 superblock if we relocate root, and invalidating directory
///     // pointers if we relocate the head of a directory. On top of
///     // this, relocations increase the overall complexity of
///     // lfs_migration, which is already a delicate operation.
/// #ifdef LFS_MIGRATE
///     if (lfs->lfs1) {
///         tired = false;
///     }
/// #endif
///
///     if (tired && lfs_pair_cmp(dir->pair, (const lfs_block_t[2]){0, 1}) != 0) {
///         // we're writing too much, time to relocate
///         goto relocate;
///     }
///
///     // begin loop to commit compaction to blocks until a compact sticks
///     while (true) {
///         {
///             // setup commit state
///             struct lfs_commit commit = {
///                 .block = dir->pair[1],
///                 .off = 0,
///                 .ptag = 0xffffffff,
///                 .crc = 0xffffffff,
///
///                 .begin = 0,
///                 .end = (lfs->cfg->metadata_max ?
///                     lfs->cfg->metadata_max : lfs->cfg->block_size) - 8,
///             };
///
///             // erase block to write to
///             int err = lfs_bd_erase(lfs, dir->pair[1]);
///             if (err) {
///                 if (err == LFS_ERR_CORRUPT) {
///                     goto relocate;
///                 }
///                 return err;
///             }
///
///             // write out header
///             dir->rev = lfs_tole32(dir->rev);
///             err = lfs_dir_commitprog(lfs, &commit,
///                     &dir->rev, sizeof(dir->rev));
///             dir->rev = lfs_fromle32(dir->rev);
///             if (err) {
///                 if (err == LFS_ERR_CORRUPT) {
///                     goto relocate;
///                 }
///                 return err;
///             }
///
///             // traverse the directory, this time writing out all unique tags
///             err = lfs_dir_traverse(lfs,
///                     source, 0, 0xffffffff, attrs, attrcount,
///                     LFS_MKTAG(0x400, 0x3ff, 0),
///                     LFS_MKTAG(LFS_TYPE_NAME, 0, 0),
///                     begin, end, -begin,
///                     lfs_dir_commit_commit, &(struct lfs_dir_commit_commit){
///                         lfs, &commit});
///             if (err) {
///                 if (err == LFS_ERR_CORRUPT) {
///                     goto relocate;
///                 }
///                 return err;
///             }
///
///             // commit tail, which may be new after last size check
///             if (!lfs_pair_isnull(dir->tail)) {
///                 lfs_pair_tole32(dir->tail);
///                 err = lfs_dir_commitattr(lfs, &commit,
///                         LFS_MKTAG(LFS_TYPE_TAIL + dir->split, 0x3ff, 8),
///                         dir->tail);
///                 lfs_pair_fromle32(dir->tail);
///                 if (err) {
///                     if (err == LFS_ERR_CORRUPT) {
///                         goto relocate;
///                     }
///                     return err;
///                 }
///             }
///
///             // bring over gstate?
///             lfs_gstate_t delta = {0};
///             if (!relocated) {
///                 lfs_gstate_xor(&delta, &lfs->gdisk);
///                 lfs_gstate_xor(&delta, &lfs->gstate);
///             }
///             lfs_gstate_xor(&delta, &lfs->gdelta);
///             delta.tag &= ~LFS_MKTAG(0, 0, 0x3ff);
///
///             err = lfs_dir_getgstate(lfs, dir, &delta);
///             if (err) {
///                 return err;
///             }
///
///             if (!lfs_gstate_iszero(&delta)) {
///                 lfs_gstate_tole32(&delta);
///                 err = lfs_dir_commitattr(lfs, &commit,
///                         LFS_MKTAG(LFS_TYPE_MOVESTATE, 0x3ff,
///                             sizeof(delta)), &delta);
///                 if (err) {
///                     if (err == LFS_ERR_CORRUPT) {
///                         goto relocate;
///                     }
///                     return err;
///                 }
///             }
///
///             // complete commit with crc
///             err = lfs_dir_commitcrc(lfs, &commit);
///             if (err) {
///                 if (err == LFS_ERR_CORRUPT) {
///                     goto relocate;
///                 }
///                 return err;
///             }
///
///             // successful compaction, swap dir pair to indicate most recent
///             LFS_ASSERT(commit.off % lfs->cfg->prog_size == 0);
///             lfs_pair_swap(dir->pair);
///             dir->count = end - begin;
///             dir->off = commit.off;
///             dir->etag = commit.ptag;
///             // update gstate
///             lfs->gdelta = (lfs_gstate_t){0};
///             if (!relocated) {
///                 lfs->gdisk = lfs->gstate;
///             }
///         }
///         break;
///
/// relocate:
///         // commit was corrupted, drop caches and prepare to relocate block
///         relocated = true;
///         lfs_cache_drop(lfs, &lfs->pcache);
///         if (!tired) {
///             LFS_DEBUG("Bad block at 0x%"PRIx32, dir->pair[1]);
///         }
///
///         // can't relocate superblock, filesystem is now frozen
///         if (lfs_pair_cmp(dir->pair, (const lfs_block_t[2]){0, 1}) == 0) {
///             LFS_WARN("Superblock 0x%"PRIx32" has become unwritable",
///                     dir->pair[1]);
///             return LFS_ERR_NOSPC;
///         }
///
///         // relocate half of pair
///         int err = lfs_alloc(lfs, &dir->pair[1]);
///         if (err && (err != LFS_ERR_NOSPC || !tired)) {
///             return err;
///         }
///
///         tired = false;
///         continue;
///     }
///
///     return relocated ? LFS_OK_RELOCATED : 0;
/// }
/// ```
pub fn lfs_dir_compact(
    lfs: *mut Lfs,
    dir: *mut LfsMdir,
    attrs: *const core::ffi::c_void,
    attrcount: i32,
    source: *const LfsMdir,
    begin: u16,
    end: u16,
) -> i32 {
    use crate::bd::bd::{lfs_bd_erase, lfs_cache_drop};
    use crate::block_alloc::alloc::{lfs_alloc, lfs_alloc_lookahead};
    use crate::dir::traverse::lfs_dir_traverse;
    use crate::error::{LFS_ERR_CORRUPT, LFS_ERR_NOSPC};
    use crate::lfs_gstate::{lfs_gstate_iszero, lfs_gstate_tole32, lfs_gstate_xor};
    use crate::tag::lfs_mktag;
    use crate::types::LFS_BLOCK_NULL;
    use crate::util::{
        lfs_fromle32, lfs_pair_cmp, lfs_pair_fromle32, lfs_pair_isnull, lfs_pair_swap,
        lfs_pair_tole32, lfs_tole32,
    };

    unsafe {
        let mut relocated = false;
        let mut relocate_count: u32 = 0;
        let mut tired = lfs_dir_needsrelocation(lfs, dir);
        let dir_ref = &mut *dir;
        let superblock_pair = [0u32, 1u32];

        dir_ref.rev = dir_ref.rev.wrapping_add(1);

        if tired && lfs_pair_cmp(&dir_ref.pair, &superblock_pair) != 0 {
            relocated = true;
            lfs_cache_drop(lfs, &mut (*lfs).pcache as *mut _);
            let err = lfs_alloc(lfs, &mut dir_ref.pair[1]);
            if err != 0 && (err != LFS_ERR_NOSPC || !tired) {
                crate::lfs_trace!(
                    "lfs_dir_compact: tired pre-alloc failed err={} pair={:?}",
                    err,
                    dir_ref.pair
                );
                return crate::lfs_pass_err!(err);
            }
            tired = false;
        }

        #[cfg(feature = "loop_limits")]
        const MAX_COMPACT_ITER: u32 = 1024;
        #[cfg(feature = "loop_limits")]
        let mut compact_iter: u32 = 0;
        loop {
            #[cfg(feature = "loop_limits")]
            {
                if compact_iter >= MAX_COMPACT_ITER {
                    panic!(
                        "loop_limits: MAX_COMPACT_ITER ({}) exceeded in lfs_dir_compact",
                        MAX_COMPACT_ITER
                    );
                }
                compact_iter += 1;
            }
            let metadata_max = (*lfs).cfg.as_ref().map_or(0, |c| c.metadata_max);
            let block_size = (*lfs).cfg.as_ref().unwrap().block_size;
            let end_off = if metadata_max != 0 {
                metadata_max
            } else {
                block_size
            } - 8;

            let mut commit = LfsCommit {
                block: dir_ref.pair[1],
                off: 0,
                ptag: 0xffff_ffff,
                crc: 0xffff_ffff,
                begin: 0,
                end: end_off,
            };

            let err = lfs_bd_erase(lfs, dir_ref.pair[1]);
            if err != 0 {
                if err == LFS_ERR_CORRUPT {
                    relocated = true;
                    relocate_count += 1;
                    crate::lfs_trace!(
                        "lfs_dir_compact relocate #{}: bd_erase CORRUPT pair={:?}",
                        relocate_count,
                        dir_ref.pair
                    );
                    lfs_alloc_lookahead(lfs, dir_ref.pair[1]);
                    lfs_cache_drop(lfs, &mut (*lfs).pcache as *mut _);
                    if lfs_pair_cmp(&dir_ref.pair, &superblock_pair) == 0 {
                        crate::lfs_trace!("lfs_dir_compact NOSPC: root+CORRUPT bd_erase");
                        return crate::lfs_err!(LFS_ERR_NOSPC);
                    }
                    let err2 = lfs_alloc(lfs, &mut dir_ref.pair[1]);
                    if err2 != 0 && (err2 != LFS_ERR_NOSPC || !tired) {
                        crate::lfs_trace!(
                            "lfs_dir_compact NOSPC: alloc failed after bd_erase err={}",
                            err2
                        );
                        return err2;
                    }
                    tired = false;
                    continue;
                }
                return crate::lfs_pass_err!(err);
            }

            let mut rev = lfs_tole32(dir_ref.rev);
            let mut err = lfs_dir_commitprog(lfs, &mut commit, &rev as *const _ as *const _, 4);
            dir_ref.rev = lfs_fromle32(rev);
            if err != 0 {
                if err == LFS_ERR_CORRUPT {
                    relocated = true;
                    relocate_count += 1;
                    crate::lfs_trace!(
                        "lfs_dir_compact relocate #{}: commitprog CORRUPT pair={:?}",
                        relocate_count,
                        dir_ref.pair
                    );
                    lfs_alloc_lookahead(lfs, dir_ref.pair[1]);
                    lfs_cache_drop(lfs, &mut (*lfs).pcache as *mut _);
                    if lfs_pair_cmp(&dir_ref.pair, &superblock_pair) == 0 {
                        crate::lfs_trace!("lfs_dir_compact NOSPC: root+CORRUPT commitprog");
                        return crate::lfs_err!(LFS_ERR_NOSPC);
                    }
                    let err2 = lfs_alloc(lfs, &mut dir_ref.pair[1]);
                    if err2 != 0 && (err2 != LFS_ERR_NOSPC || !tired) {
                        crate::lfs_trace!(
                            "lfs_dir_compact NOSPC: alloc failed after commitprog err={}",
                            err2
                        );
                        return err2;
                    }
                    tired = false;
                    continue;
                }
                return crate::lfs_pass_err!(err);
            }

            let mut commit_commit: (*mut Lfs, *mut LfsCommit) = (lfs, &mut commit as *mut _);
            err = lfs_dir_traverse(
                lfs,
                source,
                0,
                0xffff_ffff,
                attrs,
                attrcount,
                lfs_mktag(0x400, 0x3ff, 0),
                lfs_mktag(crate::lfs_type::lfs_type::LFS_TYPE_NAME, 0, 0),
                begin,
                end,
                -(begin as i16),
                Some(lfs_dir_commit_commit_raw),
                &mut commit_commit as *mut _ as *mut core::ffi::c_void,
            );
            if err != 0 {
                if err == LFS_ERR_CORRUPT {
                    relocated = true;
                    relocate_count += 1;
                    crate::lfs_trace!(
                        "lfs_dir_compact relocate #{}: traverse CORRUPT pair={:?}",
                        relocate_count,
                        dir_ref.pair
                    );
                    lfs_alloc_lookahead(lfs, dir_ref.pair[1]);
                    lfs_cache_drop(lfs, &mut (*lfs).pcache as *mut _);
                    if lfs_pair_cmp(&dir_ref.pair, &superblock_pair) == 0 {
                        crate::lfs_trace!("lfs_dir_compact NOSPC: root+err traverse");
                        return crate::lfs_err!(LFS_ERR_NOSPC);
                    }
                    let err2 = lfs_alloc(lfs, &mut dir_ref.pair[1]);
                    if err2 != 0 && (err2 != LFS_ERR_NOSPC || !tired) {
                        crate::lfs_trace!(
                            "lfs_dir_compact NOSPC: alloc failed after traverse err={}",
                            err2
                        );
                        return err2;
                    }
                    tired = false;
                    continue;
                }
                crate::lfs_trace!("lfs_dir_compact: traverse returned err={}", err);
                return crate::lfs_pass_err!(err);
            }

            if !lfs_pair_isnull(&dir_ref.tail) {
                lfs_pair_tole32(&mut dir_ref.tail);
                err = lfs_dir_commitattr(
                    lfs,
                    &mut commit as *mut _,
                    lfs_mktag(
                        crate::lfs_type::lfs_type::LFS_TYPE_TAIL
                            + if dir_ref.split { 1 } else { 0 },
                        0x3ff,
                        8,
                    ),
                    &dir_ref.tail as *const _ as *const _,
                );
                lfs_pair_fromle32(&mut dir_ref.tail);
                if err != 0 {
                    if err == LFS_ERR_CORRUPT {
                        relocated = true;
                        relocate_count += 1;
                        crate::lfs_trace!(
                            "lfs_dir_compact relocate #{}: tail CORRUPT pair={:?}",
                            relocate_count,
                            dir_ref.pair
                        );
                        lfs_alloc_lookahead(lfs, dir_ref.pair[1]);
                        lfs_cache_drop(lfs, &mut (*lfs).pcache as *mut _);
                        if lfs_pair_cmp(&dir_ref.pair, &superblock_pair) == 0 {
                            crate::lfs_trace!("lfs_dir_compact NOSPC: root+CORRUPT tail");
                            return crate::lfs_err!(LFS_ERR_NOSPC);
                        }
                        let err2 = lfs_alloc(lfs, &mut dir_ref.pair[1]);
                        if err2 != 0 && (err2 != LFS_ERR_NOSPC || !tired) {
                            crate::lfs_trace!(
                                "lfs_dir_compact NOSPC: alloc failed after tail err={}",
                                err2
                            );
                            return err2;
                        }
                        tired = false;
                        continue;
                    }
                    return crate::lfs_pass_err!(err);
                }
            }

            let mut delta = crate::lfs_gstate::LfsGstate {
                tag: 0,
                pair: [0, 0],
            };
            if !relocated {
                lfs_gstate_xor(&mut delta, &(*lfs).gdisk);
                lfs_gstate_xor(&mut delta, &(*lfs).gstate);
            }
            lfs_gstate_xor(&mut delta, &(*lfs).gdelta);
            delta.tag &= !lfs_mktag(0, 0, 0x3ff);

            err = lfs_dir_getgstate(lfs, dir, &mut delta);
            if err != 0 {
                return crate::lfs_pass_err!(err);
            }

            if !lfs_gstate_iszero(&delta) {
                lfs_gstate_tole32(&mut delta);
                err = lfs_dir_commitattr(
                    lfs,
                    &mut commit as *mut _,
                    lfs_mktag(
                        crate::lfs_type::lfs_type::LFS_TYPE_MOVESTATE,
                        0x3ff,
                        core::mem::size_of::<crate::lfs_gstate::LfsGstate>() as u32,
                    ),
                    &delta as *const _ as *const _,
                );
                if err != 0 {
                    if err == LFS_ERR_CORRUPT {
                        relocated = true;
                        relocate_count += 1;
                        crate::lfs_trace!(
                            "lfs_dir_compact relocate #{}: movestate CORRUPT pair={:?}",
                            relocate_count,
                            dir_ref.pair
                        );
                        lfs_alloc_lookahead(lfs, dir_ref.pair[1]);
                        lfs_cache_drop(lfs, &mut (*lfs).pcache as *mut _);
                        if lfs_pair_cmp(&dir_ref.pair, &superblock_pair) == 0 {
                            crate::lfs_trace!("lfs_dir_compact NOSPC: root+CORRUPT movestate");
                            return crate::lfs_err!(LFS_ERR_NOSPC);
                        }
                        let err2 = lfs_alloc(lfs, &mut dir_ref.pair[1]);
                        if err2 != 0 && (err2 != LFS_ERR_NOSPC || !tired) {
                            crate::lfs_trace!(
                                "lfs_dir_compact NOSPC: alloc failed after movestate err={}",
                                err2
                            );
                            return err2;
                        }
                        tired = false;
                        continue;
                    }
                    return crate::lfs_pass_err!(err);
                }
            }

            err = lfs_dir_commitcrc(lfs, &mut commit);
            if err != 0 {
                if err == LFS_ERR_CORRUPT {
                    relocated = true;
                    relocate_count += 1;
                    crate::lfs_trace!(
                        "lfs_dir_compact relocate #{}: commitcrc CORRUPT pair={:?}",
                        relocate_count,
                        dir_ref.pair
                    );
                    lfs_alloc_lookahead(lfs, dir_ref.pair[1]);
                    lfs_cache_drop(lfs, &mut (*lfs).pcache as *mut _);
                    if lfs_pair_cmp(&dir_ref.pair, &superblock_pair) == 0 {
                        crate::lfs_trace!("lfs_dir_compact NOSPC: root+CORRUPT commitcrc");
                        return crate::lfs_err!(LFS_ERR_NOSPC);
                    }
                    let err2 = lfs_alloc(lfs, &mut dir_ref.pair[1]);
                    if err2 != 0 && (err2 != LFS_ERR_NOSPC || !tired) {
                        crate::lfs_trace!(
                            "lfs_dir_compact NOSPC: alloc failed after commitcrc err={}",
                            err2
                        );
                        return err2;
                    }
                    tired = false;
                    continue;
                }
                return crate::lfs_pass_err!(err);
            }

            crate::lfs_assert!(commit
                .off
                .is_multiple_of((*lfs).cfg.as_ref().unwrap().prog_size));
            lfs_pair_swap(&mut dir_ref.pair);
            dir_ref.count = end - begin;
            dir_ref.off = commit.off;
            dir_ref.etag = commit.ptag;
            (*lfs).gdelta = crate::lfs_gstate::LfsGstate {
                tag: 0,
                pair: [0, 0],
            };
            if !relocated {
                (*lfs).gdisk = (*lfs).gstate;
            }
            break;
        }

        if relocated {
            crate::error::LFS_OK_RELOCATED
        } else {
            0
        }
    }
}

/// Per lfs.c lfs_dir_splittingcompact (lines 2125-2232)
///
/// C:
/// ```c
/// static int lfs_dir_splittingcompact(lfs_t *lfs, lfs_mdir_t *dir,
///         const struct lfs_mattr *attrs, int attrcount,
///         lfs_mdir_t *source, uint16_t begin, uint16_t end) {
///     while (true) {
///         // find size of first split, we do this by halving the split until
///         // the metadata is guaranteed to fit
///         //
///         // Note that this isn't a true binary search, we never increase the
///         // split size. This may result in poorly distributed metadata but isn't
///         // worth the extra code size or performance hit to fix.
///         lfs_size_t split = begin;
///         while (end - split > 1) {
///             lfs_size_t size = 0;
///             int err = lfs_dir_traverse(lfs,
///                     source, 0, 0xffffffff, attrs, attrcount,
///                     LFS_MKTAG(0x400, 0x3ff, 0),
///                     LFS_MKTAG(LFS_TYPE_NAME, 0, 0),
///                     split, end, -split,
///                     lfs_dir_commit_size, &size);
///             if (err) {
///                 return err;
///             }
///
///             // space is complicated, we need room for:
///             //
///             // - tail:         4+2*4 = 12 bytes
///             // - gstate:       4+3*4 = 16 bytes
///             // - move delete:  4     = 4 bytes
///             // - crc:          4+4   = 8 bytes
///             //                 total = 40 bytes
///             //
///             // And we cap at half a block to avoid degenerate cases with
///             // nearly-full metadata blocks.
///             //
///             lfs_size_t metadata_max = (lfs->cfg->metadata_max)
///                     ? lfs->cfg->metadata_max
///                     : lfs->cfg->block_size;
///             if (end - split < 0xff
///                     && size <= lfs_min(
///                         metadata_max - 40,
///                         lfs_alignup(
///                             metadata_max/2,
///                             lfs->cfg->prog_size))) {
///                 break;
///             }
///
///             split = split + ((end - split) / 2);
///         }
///
///         if (split == begin) {
///             // no split needed
///             break;
///         }
///
///         // split into two metadata pairs and continue
///         int err = lfs_dir_split(lfs, dir, attrs, attrcount,
///                 source, split, end);
///         if (err && err != LFS_ERR_NOSPC) {
///             return err;
///         }
///
///         if (err) {
///             // we can't allocate a new block, try to compact with degraded
///             // performance
///             LFS_WARN("Unable to split {0x%"PRIx32", 0x%"PRIx32"}",
///                     dir->pair[0], dir->pair[1]);
///             break;
///         } else {
///             end = split;
///         }
///     }
///
///     if (lfs_dir_needsrelocation(lfs, dir)
///             && lfs_pair_cmp(dir->pair, (const lfs_block_t[2]){0, 1}) == 0) {
///         // oh no! we're writing too much to the superblock,
///         // should we expand?
///         lfs_ssize_t size = lfs_fs_size_(lfs);
///         if (size < 0) {
///             return size;
///         }
///
///         // littlefs cannot reclaim expanded superblocks, so expand cautiously
///         //
///         // if our filesystem is more than ~88% full, don't expand, this is
///         // somewhat arbitrary
///         if (lfs->block_count - size > lfs->block_count/8) {
///             LFS_DEBUG("Expanding superblock at rev %"PRIu32, dir->rev);
///             int err = lfs_dir_split(lfs, dir, attrs, attrcount,
///                     source, begin, end);
///             if (err && err != LFS_ERR_NOSPC) {
///                 return err;
///             }
///
///             if (err) {
///                 // welp, we tried, if we ran out of space there's not much
///                 // we can do, we'll error later if we've become frozen
///                 LFS_WARN("Unable to expand superblock");
///             } else {
///                 // duplicate the superblock entry into the new superblock
///                 end = 1;
///             }
///         }
///     }
///
///     return lfs_dir_compact(lfs, dir, attrs, attrcount, source, begin, end);
/// }
/// ```
pub fn lfs_dir_splittingcompact(
    lfs: *mut Lfs,
    dir: *mut LfsMdir,
    attrs: *const core::ffi::c_void,
    attrcount: i32,
    source: *const LfsMdir,
    begin: u16,
    end: u16,
) -> i32 {
    use crate::dir::traverse::lfs_dir_traverse;
    use crate::tag::lfs_mktag;
    use crate::types::lfs_size_t;
    use crate::util::{lfs_alignup, lfs_min, lfs_pair_cmp};

    unsafe {
        let mut split = begin;
        let mut end_val = end;

        loop {
            while end_val - split > 1 {
                let mut size: lfs_size_t = 0;
                let mut size_ptr = size;
                let err = lfs_dir_traverse(
                    lfs,
                    source,
                    0,
                    0xffff_ffff,
                    attrs,
                    attrcount,
                    lfs_mktag(0x400, 0x3ff, 0),
                    lfs_mktag(crate::lfs_type::lfs_type::LFS_TYPE_NAME, 0, 0),
                    split,
                    end_val,
                    -(split as i16),
                    Some(lfs_dir_commit_size_raw),
                    &mut size_ptr as *mut _ as *mut core::ffi::c_void,
                );
                if err != 0 {
                    return crate::lfs_pass_err!(err);
                }
                size = size_ptr;

                let metadata_max = (*lfs).cfg.as_ref().map_or(0, |c| c.metadata_max);
                let block_size = (*lfs).cfg.as_ref().unwrap().block_size;
                let prog_size = (*lfs).cfg.as_ref().unwrap().prog_size;
                let effective_max = if metadata_max != 0 {
                    metadata_max
                } else {
                    block_size
                };
                let max_space = effective_max - 40;
                let half_block = lfs_alignup(effective_max / 2, prog_size);
                crate::lfs_trace!(
                    "splittingcompact: split={} end_val={} size={} max_space={} half_block={} break={}",
                    split,
                    end_val,
                    size,
                    max_space,
                    half_block,
                    end_val - split < 0xff && size <= lfs_min(max_space, half_block)
                );
                if end_val - split < 0xff && size <= lfs_min(max_space, half_block) {
                    break;
                }
                split = split + ((end_val - split) / 2);
            }

            if split == begin {
                crate::lfs_trace!("splittingcompact: no split needed split==begin");
                break;
            }
            if end_val <= split {
                crate::lfs_trace!(
                    "splittingcompact: skip empty range split={} end_val={}",
                    split,
                    end_val
                );
                break;
            }

            crate::lfs_trace!(
                "splittingcompact: calling dir_split split={} end_val={}",
                split,
                end_val
            );
            let err = lfs_dir_split(lfs, dir, attrs, attrcount, source, split, end_val);
            if err != 0 && err != crate::error::LFS_ERR_NOSPC {
                return crate::lfs_pass_err!(err);
            }
            if err != 0 {
                break;
            } else {
                end_val = split;
            }
        }

        let dir_ref = &*dir;
        let superblock_pair = [0u32, 1u32];
        if lfs_dir_needsrelocation(lfs, dir) && lfs_pair_cmp(&dir_ref.pair, &superblock_pair) == 0 {
            let size = lfs_fs_size_(lfs);
            if size < 0 {
                return size;
            }
            if (*lfs).block_count as i64 - size as i64 > ((*lfs).block_count as i64) / 8 {
                let err = lfs_dir_split(lfs, dir, attrs, attrcount, source, begin, end_val);
                if err != 0 && err != crate::error::LFS_ERR_NOSPC {
                    return crate::lfs_pass_err!(err);
                }
                if err == 0 {
                    end_val = 1;
                }
            }
        }

        lfs_dir_compact(lfs, dir, attrs, attrcount, source, begin, end_val)
    }
}

unsafe extern "C" fn lfs_dir_commit_size_raw(
    p: *mut core::ffi::c_void,
    tag: lfs_tag_t,
    buffer: *const core::ffi::c_void,
) -> i32 {
    lfs_dir_commit_size(p, tag, buffer)
}

/// Per lfs.c lfs_dir_relocatingcommit (lines 2234-2406)
///
/// C:
/// ```c
/// static int lfs_dir_relocatingcommit(lfs_t *lfs, lfs_mdir_t *dir,
///         const lfs_block_t pair[2],
///         const struct lfs_mattr *attrs, int attrcount,
///         lfs_mdir_t *pdir) {
///     int state = 0;
///
///     // calculate changes to the directory
///     bool hasdelete = false;
///     for (int i = 0; i < attrcount; i++) {
///         if (lfs_tag_type3(attrs[i].tag) == LFS_TYPE_CREATE) {
///             dir->count += 1;
///         } else if (lfs_tag_type3(attrs[i].tag) == LFS_TYPE_DELETE) {
///             LFS_ASSERT(dir->count > 0);
///             dir->count -= 1;
///             hasdelete = true;
///         } else if (lfs_tag_type1(attrs[i].tag) == LFS_TYPE_TAIL) {
///             dir->tail[0] = ((lfs_block_t*)attrs[i].buffer)[0];
///             dir->tail[1] = ((lfs_block_t*)attrs[i].buffer)[1];
///             dir->split = (lfs_tag_chunk(attrs[i].tag) & 1);
///             lfs_pair_fromle32(dir->tail);
///         }
///     }
///
///     // should we actually drop the directory block?
///     if (hasdelete && dir->count == 0) {
///         LFS_ASSERT(pdir);
///         int err = lfs_fs_pred(lfs, dir->pair, pdir);
///         if (err && err != LFS_ERR_NOENT) {
///             return err;
///         }
///
///         if (err != LFS_ERR_NOENT && pdir->split) {
///             state = LFS_OK_DROPPED;
///             goto fixmlist;
///         }
///     }
///
///     if (dir->erased && dir->count < 0xff) {
///         // try to commit
///         struct lfs_commit commit = {
///             .block = dir->pair[0],
///             .off = dir->off,
///             .ptag = dir->etag,
///             .crc = 0xffffffff,
///
///             .begin = dir->off,
///             .end = (lfs->cfg->metadata_max ?
///                 lfs->cfg->metadata_max : lfs->cfg->block_size) - 8,
///         };
///
///         // traverse attrs that need to be written out
///         lfs_pair_tole32(dir->tail);
///         int err = lfs_dir_traverse(lfs,
///                 dir, dir->off, dir->etag, attrs, attrcount,
///                 0, 0, 0, 0, 0,
///                 lfs_dir_commit_commit, &(struct lfs_dir_commit_commit){
///                     lfs, &commit});
///         lfs_pair_fromle32(dir->tail);
///         if (err) {
///             if (err == LFS_ERR_NOSPC || err == LFS_ERR_CORRUPT) {
///                 goto compact;
///             }
///             return err;
///         }
///
///         // commit any global diffs if we have any
///         lfs_gstate_t delta = {0};
///         lfs_gstate_xor(&delta, &lfs->gstate);
///         lfs_gstate_xor(&delta, &lfs->gdisk);
///         lfs_gstate_xor(&delta, &lfs->gdelta);
///         delta.tag &= ~LFS_MKTAG(0, 0, 0x3ff);
///         if (!lfs_gstate_iszero(&delta)) {
///             err = lfs_dir_getgstate(lfs, dir, &delta);
///             if (err) {
///                 return err;
///             }
///
///             lfs_gstate_tole32(&delta);
///             err = lfs_dir_commitattr(lfs, &commit,
///                     LFS_MKTAG(LFS_TYPE_MOVESTATE, 0x3ff,
///                         sizeof(delta)), &delta);
///             if (err) {
///                 if (err == LFS_ERR_NOSPC || err == LFS_ERR_CORRUPT) {
///                     goto compact;
///                 }
///                 return err;
///             }
///         }
///
///         // finalize commit with the crc
///         err = lfs_dir_commitcrc(lfs, &commit);
///         if (err) {
///             if (err == LFS_ERR_NOSPC || err == LFS_ERR_CORRUPT) {
///                 goto compact;
///             }
///             return err;
///         }
///
///         // successful commit, update dir
///         LFS_ASSERT(commit.off % lfs->cfg->prog_size == 0);
///         dir->off = commit.off;
///         dir->etag = commit.ptag;
///         // and update gstate
///         lfs->gdisk = lfs->gstate;
///         lfs->gdelta = (lfs_gstate_t){0};
///
///         goto fixmlist;
///     }
///
/// compact:
///     // fall back to compaction
///     lfs_cache_drop(lfs, &lfs->pcache);
///
///     state = lfs_dir_splittingcompact(lfs, dir, attrs, attrcount,
///             dir, 0, dir->count);
///     if (state < 0) {
///         return state;
///     }
///
///     goto fixmlist;
///
/// fixmlist:;
///     // this complicated bit of logic is for fixing up any active
///     // metadata-pairs that we may have affected
///     //
///     // note we have to make two passes since the mdir passed to
///     // lfs_dir_commit could also be in this list, and even then
///     // we need to copy the pair so they don't get clobbered if we refetch
///     // our mdir.
///     lfs_block_t oldpair[2] = {pair[0], pair[1]};
///     for (struct lfs_mlist *d = lfs->mlist; d; d = d->next) {
///         if (lfs_pair_cmp(d->m.pair, oldpair) == 0) {
///             d->m = *dir;
///             if (d->m.pair != pair) {
///                 for (int i = 0; i < attrcount; i++) {
///                     if (lfs_tag_type3(attrs[i].tag) == LFS_TYPE_DELETE &&
///                             d->id == lfs_tag_id(attrs[i].tag) &&
///                             d->type != LFS_TYPE_DIR) {
///                         d->m.pair[0] = LFS_BLOCK_NULL;
///                         d->m.pair[1] = LFS_BLOCK_NULL;
///                     } else if (lfs_tag_type3(attrs[i].tag) == LFS_TYPE_DELETE &&
///                             d->id > lfs_tag_id(attrs[i].tag)) {
///                         d->id -= 1;
///                         if (d->type == LFS_TYPE_DIR) {
///                             ((lfs_dir_t*)d)->pos -= 1;
///                         }
///                     } else if (lfs_tag_type3(attrs[i].tag) == LFS_TYPE_CREATE &&
///                             d->id >= lfs_tag_id(attrs[i].tag)) {
///                         d->id += 1;
///                         if (d->type == LFS_TYPE_DIR) {
///                             ((lfs_dir_t*)d)->pos += 1;
///                         }
///                     }
///                 }
///             }
///
///             while (d->id >= d->m.count && d->m.split) {
///                 // we split and id is on tail now
///                 if (lfs_pair_cmp(d->m.tail, lfs->root) != 0) {
///                     d->id -= d->m.count;
///                 }
///                 int err = lfs_dir_fetch(lfs, &d->m, d->m.tail);
///                 if (err) {
///                     return err;
///                 }
///             }
///         }
///     }
///
///     return state;
/// }
/// ```
pub fn lfs_dir_relocatingcommit(
    lfs: *mut Lfs,
    dir: *mut LfsMdir,
    pair: *const [lfs_block_t; 2],
    attrs: *const core::ffi::c_void,
    attrcount: i32,
    pdir: *mut LfsMdir,
) -> i32 {
    use crate::bd::bd::lfs_cache_drop;
    use crate::dir::traverse::lfs_dir_traverse;
    use crate::error::{LFS_ERR_CORRUPT, LFS_ERR_NOSPC};
    use crate::lfs_gstate::{lfs_gstate_iszero, lfs_gstate_tole32, lfs_gstate_xor};
    use crate::lfs_type::lfs_type::{LFS_TYPE_CREATE, LFS_TYPE_DELETE, LFS_TYPE_TAIL};
    use crate::tag::{lfs_mktag, lfs_tag_type1, lfs_tag_type3};
    use crate::types::LFS_BLOCK_NULL;
    use crate::util::{lfs_pair_cmp, lfs_pair_fromle32, lfs_pair_tole32};

    unsafe {
        let mut state = 0i32;
        let dir_ref = &mut *dir;
        let pair_ref = &*pair;

        let attrs_slice = if attrcount > 0 && !attrs.is_null() {
            core::slice::from_raw_parts(attrs as *const crate::tag::lfs_mattr, attrcount as usize)
        } else {
            &[]
        };

        let mut hasdelete = false;
        for attr in attrs_slice.iter() {
            let tag = attr.tag;
            if u32::from(lfs_tag_type3(tag)) == LFS_TYPE_CREATE {
                dir_ref.count = dir_ref.count.wrapping_add(1);
            } else if u32::from(lfs_tag_type3(tag)) == LFS_TYPE_DELETE {
                crate::lfs_assert!(dir_ref.count > 0);
                dir_ref.count -= 1;
                hasdelete = true;
            } else if u32::from(lfs_tag_type1(tag)) == LFS_TYPE_TAIL {
                let buf = attr.buffer as *const [lfs_block_t; 2];
                if !buf.is_null() {
                    dir_ref.tail[0] = (*buf)[0];
                    dir_ref.tail[1] = (*buf)[1];
                }
                dir_ref.split = (crate::tag::lfs_tag_chunk(tag) & 1) != 0;
                lfs_pair_fromle32(&mut dir_ref.tail);
            }
        }

        // C: lfs.c:2257-2268
        if hasdelete && dir_ref.count == 0 {
            crate::lfs_assert!(!pdir.is_null());
            let err = crate::fs::parent::lfs_fs_pred(lfs, &dir_ref.pair, pdir);
            if err != 0 && err != crate::error::LFS_ERR_NOENT {
                return crate::lfs_pass_err!(err);
            }
            if err != crate::error::LFS_ERR_NOENT && (*pdir).split {
                state = crate::error::LFS_OK_DROPPED;
            }
        }

        // C: goto fixmlist skips the commit/compact section when DROPPED
        if state == crate::error::LFS_OK_DROPPED {
            return relocatingcommit_fixmlist(lfs, dir, pair, attrs, attrcount, state);
        }

        let mut do_compact = true;
        if dir_ref.erased && dir_ref.count < 0xff {
            let metadata_max = (*lfs).cfg.as_ref().map_or(0, |c| c.metadata_max);
            let block_size = (*lfs).cfg.as_ref().unwrap().block_size;
            let end = if metadata_max != 0 {
                metadata_max
            } else {
                block_size
            } - 8;

            let mut commit = LfsCommit {
                block: dir_ref.pair[0],
                off: dir_ref.off,
                ptag: dir_ref.etag,
                crc: 0xffff_ffff,
                begin: dir_ref.off,
                end,
            };

            lfs_pair_tole32(&mut dir_ref.tail);
            let mut commit_commit: (*mut Lfs, *mut LfsCommit) = (lfs, &mut commit as *mut _);
            let err = lfs_dir_traverse(
                lfs,
                dir,
                dir_ref.off,
                dir_ref.etag,
                attrs,
                attrcount,
                0,
                0,
                0,
                0,
                0,
                Some(lfs_dir_commit_commit_raw),
                &mut commit_commit as *mut _ as *mut core::ffi::c_void,
            );
            lfs_pair_fromle32(&mut dir_ref.tail);
            if err == 0 {
                do_compact = false;
                let mut delta = crate::lfs_gstate::LfsGstate {
                    tag: 0,
                    pair: [0, 0],
                };
                lfs_gstate_xor(&mut delta, &(*lfs).gstate);
                lfs_gstate_xor(&mut delta, &(*lfs).gdisk);
                lfs_gstate_xor(&mut delta, &(*lfs).gdelta);
                delta.tag &= !lfs_mktag(0, 0, 0x3ff);
                if !lfs_gstate_iszero(&delta) {
                    let err2 = lfs_dir_getgstate(lfs, dir, &mut delta);
                    if err2 != 0 {
                        return err2;
                    }
                    lfs_gstate_tole32(&mut delta);
                    let movestate_tag = lfs_mktag(
                        crate::lfs_type::lfs_type::LFS_TYPE_MOVESTATE,
                        0x3ff,
                        core::mem::size_of::<crate::lfs_gstate::LfsGstate>() as u32,
                    );
                    let err2 = lfs_dir_commitattr(
                        lfs,
                        &mut commit as *mut _,
                        movestate_tag,
                        &delta as *const _ as *const _,
                    );
                    if err2 != 0 {
                        if err2 == LFS_ERR_NOSPC || err2 == LFS_ERR_CORRUPT {
                            do_compact = true;
                        } else {
                            return err2;
                        }
                    }
                }
                if !do_compact {
                    let err2 = lfs_dir_commitcrc(lfs, &mut commit);
                    if err2 != 0 {
                        if err2 == LFS_ERR_NOSPC || err2 == LFS_ERR_CORRUPT {
                            do_compact = true;
                        } else {
                            return err2;
                        }
                    } else {
                        dir_ref.off = commit.off;
                        dir_ref.etag = commit.ptag;
                        (*lfs).gdisk = (*lfs).gstate;
                        (*lfs).gdelta = crate::lfs_gstate::LfsGstate {
                            tag: 0,
                            pair: [0, 0],
                        };
                    }
                }
            } else if err == LFS_ERR_NOSPC || err == LFS_ERR_CORRUPT {
                do_compact = true;
            } else {
                return crate::lfs_pass_err!(err);
            }
        }

        if do_compact {
            lfs_cache_drop(lfs, &mut (*lfs).pcache as *mut _);
            state = lfs_dir_splittingcompact(lfs, dir, attrs, attrcount, dir, 0, (*dir).count);
            if state < 0 {
                return state;
            }
        }

        relocatingcommit_fixmlist(lfs, dir, pair, attrs, attrcount, state)
    }
}

#[inline(never)]
fn relocatingcommit_fixmlist(
    lfs: *mut Lfs,
    dir: *mut LfsMdir,
    pair: *const [lfs_block_t; 2],
    attrs: *const core::ffi::c_void,
    attrcount: i32,
    state: i32,
) -> i32 {
    use crate::dir::fetch::lfs_dir_fetch;
    use crate::lfs_type::lfs_type::{LFS_TYPE_CREATE, LFS_TYPE_DELETE};
    use crate::tag::{lfs_tag_id, lfs_tag_type3};
    use crate::types::LFS_BLOCK_NULL;
    use crate::util::lfs_pair_cmp;

    unsafe {
        let oldpair = [(*pair)[0], (*pair)[1]];
        let mut d = (*lfs).mlist;
        #[cfg(feature = "loop_limits")]
        const MAX_MLIST_COMMIT_ITER: u32 = 128;
        #[cfg(feature = "loop_limits")]
        let mut mlist_iter: u32 = 0;
        while !d.is_null() {
            #[cfg(feature = "loop_limits")]
            {
                if mlist_iter >= MAX_MLIST_COMMIT_ITER {
                    panic!(
                        "loop_limits: MAX_MLIST_COMMIT_ITER ({}) exceeded",
                        MAX_MLIST_COMMIT_ITER
                    );
                }
                mlist_iter += 1;
            }
            let d_ref = &mut *d;
            if lfs_pair_cmp(&d_ref.m.pair, &oldpair) == 0 {
                d_ref.m = *dir;
                if !core::ptr::eq(&d_ref.m.pair as *const _, pair as *const _) {
                    let attrs_slice = if attrcount > 0 && !attrs.is_null() {
                        core::slice::from_raw_parts(
                            attrs as *const crate::tag::lfs_mattr,
                            attrcount as usize,
                        )
                    } else {
                        &[]
                    };
                    for attr in attrs_slice.iter() {
                        let tag = attr.tag;
                        if u32::from(lfs_tag_type3(tag)) == LFS_TYPE_DELETE
                            && d_ref.id == lfs_tag_id(tag)
                            && d_ref.type_ != crate::lfs_type::lfs_type::LFS_TYPE_DIR as u8
                        {
                            d_ref.m.pair = [LFS_BLOCK_NULL, LFS_BLOCK_NULL];
                        } else if u32::from(lfs_tag_type3(tag)) == LFS_TYPE_DELETE
                            && d_ref.id > lfs_tag_id(tag)
                        {
                            d_ref.id -= 1;
                        } else if u32::from(lfs_tag_type3(tag)) == LFS_TYPE_CREATE
                            && d_ref.id >= lfs_tag_id(tag)
                        {
                            d_ref.id = d_ref.id.wrapping_add(1);
                        }
                    }
                }
                #[cfg(feature = "loop_limits")]
                const MAX_COMMIT_DIR_ADVANCE: u32 = 2048;
                #[cfg(feature = "loop_limits")]
                let mut advance_iter: u32 = 0;
                while d_ref.id >= d_ref.m.count && d_ref.m.split {
                    #[cfg(feature = "loop_limits")]
                    {
                        if advance_iter >= MAX_COMMIT_DIR_ADVANCE {
                            panic!(
                                "loop_limits: MAX_COMMIT_DIR_ADVANCE ({}) exceeded",
                                MAX_COMMIT_DIR_ADVANCE
                            );
                        }
                        advance_iter += 1;
                    }
                    if lfs_pair_cmp(&d_ref.m.tail, &(*lfs).root) != 0 {
                        d_ref.id -= d_ref.m.count;
                    }
                    let err = lfs_dir_fetch(lfs, &mut d_ref.m, &d_ref.m.tail);
                    if err != 0 {
                        return crate::lfs_pass_err!(err);
                    }
                }
            }
            d = d_ref.next;
        }
        state
    }
}

unsafe extern "C" fn lfs_dir_commit_commit_raw(
    p: *mut core::ffi::c_void,
    tag: lfs_tag_t,
    buffer: *const core::ffi::c_void,
) -> i32 {
    crate::lfs_trace!(
        "commit_commit_raw: tag=0x{:08x} type1={} buffer={:p}",
        tag,
        crate::tag::lfs_tag_type1(tag),
        buffer
    );
    if u32::from(crate::tag::lfs_tag_type1(tag)) == crate::lfs_type::lfs_type::LFS_TYPE_SUPERBLOCK {
        let preview: [u8; 8] = if buffer.is_null() {
            [0u8; 8]
        } else {
            unsafe { core::ptr::read(buffer as *const [u8; 8]) }
        };
        crate::lfs_trace!(
            "commit_commit_raw SUPERBLOCK: buffer={:p} first 8 bytes: {:?}",
            buffer,
            preview
        );
    }
    let commit_commit = &*(p as *const (*mut Lfs, *mut LfsCommit));
    let (lfs, commit) = *commit_commit;
    lfs_dir_commitattr(lfs, commit, tag, buffer)
}

/// Per lfs.c lfs_dir_orphaningcommit (lines 2408-2599)
///
/// C:
/// ```c
/// static int lfs_dir_orphaningcommit(lfs_t *lfs, lfs_mdir_t *dir,
///         const struct lfs_mattr *attrs, int attrcount) {
///     // check for any inline files that aren't RAM backed and
///     // forcefully evict them, needed for filesystem consistency
///     for (lfs_file_t *f = (lfs_file_t*)lfs->mlist; f; f = f->next) {
///         if (dir != &f->m && lfs_pair_cmp(f->m.pair, dir->pair) == 0 &&
///                 f->type == LFS_TYPE_REG && (f->flags & LFS_F_INLINE) &&
///                 f->ctz.size > lfs->cfg->cache_size) {
///             int err = lfs_file_outline(lfs, f);
///             if (err) {
///                 return err;
///             }
///
///             err = lfs_file_flush(lfs, f);
///             if (err) {
///                 return err;
///             }
///         }
///     }
///
///     lfs_block_t lpair[2] = {dir->pair[0], dir->pair[1]};
///     lfs_mdir_t ldir = *dir;
///     lfs_mdir_t pdir;
///     int state = lfs_dir_relocatingcommit(lfs, &ldir, dir->pair,
///             attrs, attrcount, &pdir);
///     if (state < 0) {
///         return state;
///     }
///
///     // update if we're not in mlist, note we may have already been
///     // updated if we are in mlist
///     if (lfs_pair_cmp(dir->pair, lpair) == 0) {
///         *dir = ldir;
///     }
///
///     // commit was successful, but may require other changes in the
///     // filesystem, these would normally be tail recursive, but we have
///     // flattened them here avoid unbounded stack usage
///
///     // need to drop?
///     if (state == LFS_OK_DROPPED) {
///         // steal state
///         int err = lfs_dir_getgstate(lfs, dir, &lfs->gdelta);
///         if (err) {
///             return err;
///         }
///
///         // steal tail, note that this can't create a recursive drop
///         lpair[0] = pdir.pair[0];
///         lpair[1] = pdir.pair[1];
///         lfs_pair_tole32(dir->tail);
///         state = lfs_dir_relocatingcommit(lfs, &pdir, lpair, LFS_MKATTRS(
///                     {LFS_MKTAG(LFS_TYPE_TAIL + dir->split, 0x3ff, 8),
///                         dir->tail}),
///                 NULL);
///         lfs_pair_fromle32(dir->tail);
///         if (state < 0) {
///             return state;
///         }
///
///         ldir = pdir;
///     }
///
///     // need to relocate?
///     bool orphans = false;
///     while (state == LFS_OK_RELOCATED) {
///         LFS_DEBUG("Relocating {0x%"PRIx32", 0x%"PRIx32"} "
///                     "-> {0x%"PRIx32", 0x%"PRIx32"}",
///                 lpair[0], lpair[1], ldir.pair[0], ldir.pair[1]);
///         state = 0;
///
///         // update internal root
///         if (lfs_pair_cmp(lpair, lfs->root) == 0) {
///             lfs->root[0] = ldir.pair[0];
///             lfs->root[1] = ldir.pair[1];
///         }
///
///         // update internally tracked dirs
///         for (struct lfs_mlist *d = lfs->mlist; d; d = d->next) {
///             if (lfs_pair_cmp(lpair, d->m.pair) == 0) {
///                 d->m.pair[0] = ldir.pair[0];
///                 d->m.pair[1] = ldir.pair[1];
///             }
///
///             if (d->type == LFS_TYPE_DIR &&
///                     lfs_pair_cmp(lpair, ((lfs_dir_t*)d)->head) == 0) {
///                 ((lfs_dir_t*)d)->head[0] = ldir.pair[0];
///                 ((lfs_dir_t*)d)->head[1] = ldir.pair[1];
///             }
///         }
///
///         // find parent
///         lfs_stag_t tag = lfs_fs_parent(lfs, lpair, &pdir);
///         if (tag < 0 && tag != LFS_ERR_NOENT) {
///             return tag;
///         }
///
///         bool hasparent = (tag != LFS_ERR_NOENT);
///         if (tag != LFS_ERR_NOENT) {
///             // note that if we have a parent, we must have a pred, so this will
///             // always create an orphan
///             int err = lfs_fs_preporphans(lfs, +1);
///             if (err) {
///                 return err;
///             }
///
///             // fix pending move in this pair? this looks like an optimization but
///             // is in fact _required_ since relocating may outdate the move.
///             uint16_t moveid = 0x3ff;
///             if (lfs_gstate_hasmovehere(&lfs->gstate, pdir.pair)) {
///                 moveid = lfs_tag_id(lfs->gstate.tag);
///                 LFS_DEBUG("Fixing move while relocating "
///                         "{0x%"PRIx32", 0x%"PRIx32"} 0x%"PRIx16"\n",
///                         pdir.pair[0], pdir.pair[1], moveid);
///                 lfs_fs_prepmove(lfs, 0x3ff, NULL);
///                 if (moveid < lfs_tag_id(tag)) {
///                     tag -= LFS_MKTAG(0, 1, 0);
///                 }
///             }
///
///             lfs_block_t ppair[2] = {pdir.pair[0], pdir.pair[1]};
///             lfs_pair_tole32(ldir.pair);
///             state = lfs_dir_relocatingcommit(lfs, &pdir, ppair, LFS_MKATTRS(
///                         {LFS_MKTAG_IF(moveid != 0x3ff,
///                             LFS_TYPE_DELETE, moveid, 0), NULL},
///                         {tag, ldir.pair}),
///                     NULL);
///             lfs_pair_fromle32(ldir.pair);
///             if (state < 0) {
///                 return state;
///             }
///
///             if (state == LFS_OK_RELOCATED) {
///                 lpair[0] = ppair[0];
///                 lpair[1] = ppair[1];
///                 ldir = pdir;
///                 orphans = true;
///                 continue;
///             }
///         }
///
///         // find pred
///         int err = lfs_fs_pred(lfs, lpair, &pdir);
///         if (err && err != LFS_ERR_NOENT) {
///             return err;
///         }
///         LFS_ASSERT(!(hasparent && err == LFS_ERR_NOENT));
///
///         // if we can't find dir, it must be new
///         if (err != LFS_ERR_NOENT) {
///             if (lfs_gstate_hasorphans(&lfs->gstate)) {
///                 // next step, clean up orphans
///                 err = lfs_fs_preporphans(lfs, -(int8_t)hasparent);
///                 if (err) {
///                     return err;
///                 }
///             }
///
///             // fix pending move in this pair? this looks like an optimization
///             // but is in fact _required_ since relocating may outdate the move.
///             uint16_t moveid = 0x3ff;
///             if (lfs_gstate_hasmovehere(&lfs->gstate, pdir.pair)) {
///                 moveid = lfs_tag_id(lfs->gstate.tag);
///                 LFS_DEBUG("Fixing move while relocating "
///                         "{0x%"PRIx32", 0x%"PRIx32"} 0x%"PRIx16"\n",
///                         pdir.pair[0], pdir.pair[1], moveid);
///                 lfs_fs_prepmove(lfs, 0x3ff, NULL);
///             }
///
///             // replace bad pair, either we clean up desync, or no desync occured
///             lpair[0] = pdir.pair[0];
///             lpair[1] = pdir.pair[1];
///             lfs_pair_tole32(ldir.pair);
///             state = lfs_dir_relocatingcommit(lfs, &pdir, lpair, LFS_MKATTRS(
///                         {LFS_MKTAG_IF(moveid != 0x3ff,
///                             LFS_TYPE_DELETE, moveid, 0), NULL},
///                         {LFS_MKTAG(LFS_TYPE_TAIL + pdir.split, 0x3ff, 8),
///                             ldir.pair}),
///                     NULL);
///             lfs_pair_fromle32(ldir.pair);
///             if (state < 0) {
///                 return state;
///             }
///
///             ldir = pdir;
///         }
///     }
///
///     return orphans ? LFS_OK_ORPHANED : 0;
/// }
/// ```
pub fn lfs_dir_orphaningcommit(
    lfs: *mut crate::fs::Lfs,
    dir: *mut LfsMdir,
    attrs: *const core::ffi::c_void,
    attrcount: i32,
) -> i32 {
    use crate::error::LFS_OK_ORPHANED;
    use crate::util::{lfs_pair_cmp, lfs_pair_fromle32, lfs_pair_tole32};

    unsafe {
        let lpair = (*dir).pair;
        let mut ldir = *dir;
        let mut pdir = core::mem::zeroed();

        let state =
            lfs_dir_relocatingcommit(lfs, &mut ldir, &(*dir).pair, attrs, attrcount, &mut pdir);
        if state < 0 {
            return state;
        }

        if lfs_pair_cmp(&(*dir).pair, &lpair) == 0 {
            *dir = ldir;
        }

        if state == crate::error::LFS_OK_DROPPED {
            let err = lfs_dir_getgstate(lfs, dir, &mut (*lfs).gdelta);
            if err != 0 {
                return crate::lfs_pass_err!(err);
            }
            let plpair = [pdir.pair[0], pdir.pair[1]];
            lfs_pair_tole32(&mut (*dir).tail);
            let tail_attrs = [crate::tag::lfs_mattr {
                tag: crate::tag::lfs_mktag(
                    crate::lfs_type::lfs_type::LFS_TYPE_TAIL + if (*dir).split { 1 } else { 0 },
                    0x3ff,
                    8,
                ),
                buffer: (*dir).tail.as_ptr() as *const core::ffi::c_void,
            }];
            let tail_state = lfs_dir_relocatingcommit(
                lfs,
                &mut pdir,
                &plpair,
                tail_attrs.as_ptr() as *const _,
                1,
                core::ptr::null_mut(),
            );
            lfs_pair_fromle32(&mut (*dir).tail);
            if tail_state < 0 {
                return tail_state;
            }
            ldir = pdir;
        }

        // C: lfs.c:2472-2594 — relocation handling
        let mut orphans = false;
        let mut state = state;
        let mut lpair = lpair;
        #[cfg(feature = "loop_limits")]
        const MAX_RELOCATE_ITER: u32 = 512;
        #[cfg(feature = "loop_limits")]
        let mut relocate_iter: u32 = 0;
        while state == crate::error::LFS_OK_RELOCATED {
            #[cfg(feature = "loop_limits")]
            {
                if relocate_iter >= MAX_RELOCATE_ITER {
                    panic!(
                        "loop_limits: MAX_RELOCATE_ITER ({}) exceeded",
                        MAX_RELOCATE_ITER
                    );
                }
                relocate_iter += 1;
            }
            state = 0;

            // C: lfs.c:2480-2483 — update internal root
            if lfs_pair_cmp(&lpair, &(*lfs).root) == 0 {
                (*lfs).root[0] = ldir.pair[0];
                (*lfs).root[1] = ldir.pair[1];
            }

            // C: lfs.c:2486-2497 — update internally tracked dirs
            {
                let mut d = (*lfs).mlist;
                while !d.is_null() {
                    if lfs_pair_cmp(&lpair, &(*d).m.pair) == 0 {
                        (*d).m.pair[0] = ldir.pair[0];
                        (*d).m.pair[1] = ldir.pair[1];
                    }
                    d = (*d).next;
                }
            }

            // C: lfs.c:2500-2547 — find parent and update
            let mut tag = crate::fs::parent::lfs_fs_parent(lfs, &lpair, &mut pdir);
            if tag < 0 && tag != crate::error::LFS_ERR_NOENT {
                return tag;
            }

            let hasparent = tag != crate::error::LFS_ERR_NOENT;
            if hasparent {
                let err = crate::fs::superblock::lfs_fs_preporphans(lfs, 1);
                if err != 0 {
                    return crate::lfs_pass_err!(err);
                }

                let mut moveid: u16 = 0x3ff;
                if crate::lfs_gstate::lfs_gstate_hasmovehere(&(*lfs).gstate, &pdir.pair) {
                    moveid = crate::tag::lfs_tag_id((*lfs).gstate.tag);
                    crate::fs::superblock::lfs_fs_prepmove(lfs, 0x3ff, core::ptr::null());
                    // C: lfs.c:2523-2525
                    if moveid < crate::tag::lfs_tag_id(tag as u32) {
                        tag -= crate::tag::lfs_mktag(0, 1, 0) as i32;
                    }
                }

                let ppair = [pdir.pair[0], pdir.pair[1]];
                lfs_pair_tole32(&mut ldir.pair);
                let relocate_attrs = [
                    crate::tag::lfs_mattr {
                        tag: crate::tag::lfs_mktag_if(
                            moveid != 0x3ff,
                            crate::lfs_type::lfs_type::LFS_TYPE_DELETE,
                            moveid.into(),
                            0,
                        ),
                        buffer: core::ptr::null(),
                    },
                    crate::tag::lfs_mattr {
                        tag: tag as lfs_tag_t,
                        buffer: ldir.pair.as_ptr() as *const core::ffi::c_void,
                    },
                ];
                state = lfs_dir_relocatingcommit(
                    lfs,
                    &mut pdir,
                    &ppair,
                    relocate_attrs.as_ptr() as *const _,
                    2,
                    core::ptr::null_mut(),
                );
                lfs_pair_fromle32(&mut ldir.pair);
                if state < 0 {
                    return state;
                }
                if state == crate::error::LFS_OK_RELOCATED {
                    lpair = ppair;
                    ldir = pdir;
                    orphans = true;
                    continue;
                }
            }

            // C: lfs.c:2549-2593 — find pred and update tail (INSIDE the while loop)
            let err = crate::fs::parent::lfs_fs_pred(lfs, &lpair, &mut pdir);
            if err != 0 && err != crate::error::LFS_ERR_NOENT {
                return crate::lfs_pass_err!(err);
            }
            crate::lfs_assert!(!(hasparent && err == crate::error::LFS_ERR_NOENT));

            if err != crate::error::LFS_ERR_NOENT {
                if crate::lfs_gstate::lfs_gstate_hasorphans(&(*lfs).gstate) {
                    let deorphan_delta = if hasparent { -1 } else { 0 };
                    let err2 = crate::fs::superblock::lfs_fs_preporphans(lfs, deorphan_delta);
                    if err2 != 0 {
                        return err2;
                    }
                }

                let mut moveid: u16 = 0x3ff;
                if crate::lfs_gstate::lfs_gstate_hasmovehere(&(*lfs).gstate, &pdir.pair) {
                    moveid = crate::tag::lfs_tag_id((*lfs).gstate.tag);
                    crate::fs::superblock::lfs_fs_prepmove(lfs, 0x3ff, core::ptr::null());
                }

                lpair[0] = pdir.pair[0];
                lpair[1] = pdir.pair[1];
                lfs_pair_tole32(&mut ldir.pair);
                let tail_attrs = [
                    crate::tag::lfs_mattr {
                        tag: crate::tag::lfs_mktag_if(
                            moveid != 0x3ff,
                            crate::lfs_type::lfs_type::LFS_TYPE_DELETE,
                            moveid.into(),
                            0,
                        ),
                        buffer: core::ptr::null(),
                    },
                    crate::tag::lfs_mattr {
                        tag: crate::tag::lfs_mktag(
                            crate::lfs_type::lfs_type::LFS_TYPE_TAIL
                                + if pdir.split { 1 } else { 0 },
                            0x3ff,
                            8,
                        ),
                        buffer: ldir.pair.as_ptr() as *const core::ffi::c_void,
                    },
                ];
                state = lfs_dir_relocatingcommit(
                    lfs,
                    &mut pdir,
                    &lpair,
                    tail_attrs.as_ptr() as *const _,
                    2,
                    core::ptr::null_mut(),
                );
                lfs_pair_fromle32(&mut ldir.pair);
                if state < 0 {
                    return state;
                }

                ldir = pdir;
            }
        }

        if orphans {
            LFS_OK_ORPHANED
        } else {
            0
        }
    }
}

/// Per lfs.c lfs_dir_commit (lines 2601-2623)
///
/// C:
/// ```c
/// static int lfs_dir_commit(lfs_t *lfs, lfs_mdir_t *dir,
///         const struct lfs_mattr *attrs, int attrcount) {
///     int orphans = lfs_dir_orphaningcommit(lfs, dir, attrs, attrcount);
///     if (orphans < 0) {
///         return orphans;
///     }
///
///     if (orphans) {
///         int err = lfs_fs_deorphan(lfs, false);
///         if (err) return err;
///     }
///
///     return 0;
/// }
/// ```
pub fn lfs_dir_commit(
    lfs: *mut crate::fs::Lfs,
    dir: *mut LfsMdir,
    attrs: *const core::ffi::c_void,
    attrcount: i32,
) -> i32 {
    use crate::error::LFS_OK_ORPHANED;
    use crate::fs::superblock::lfs_fs_deorphan;

    unsafe {
        let orphans = lfs_dir_orphaningcommit(lfs, dir, attrs, attrcount);
        if orphans < 0 {
            return orphans;
        }

        if orphans == LFS_OK_ORPHANED {
            let err = lfs_fs_deorphan(lfs, false);
            if err != 0 {
                return crate::lfs_pass_err!(err);
            }
        }

        0
    }
}
