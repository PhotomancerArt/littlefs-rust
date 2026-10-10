//! Directory fetch. Per lfs.c lfs_dir_fetch, lfs_dir_getgstate, lfs_dir_getinfo.

use crate::bd::bd::{lfs_bd_crc, lfs_bd_read};
use crate::crc::lfs_crc;
use crate::dir::lfs_fcrc::lfs_fcrc_fromle32;
use crate::dir::traverse::lfs_dir_get;
use crate::dir::LfsFcrc;
use crate::dir::LfsMdir;
use crate::error::LFS_ERR_CORRUPT;
use crate::file::lfs_ctz::{lfs_ctz_fromle32, LfsCtz};
use crate::lfs_gstate::LfsGstate;
use crate::lfs_gstate::{lfs_gstate_fromle32, lfs_gstate_hasmovehere, lfs_gstate_xor};
use crate::lfs_info::LfsInfo;
use crate::lfs_type::lfs_type::{
    LFS_TYPE_CCRC, LFS_TYPE_CTZSTRUCT, LFS_TYPE_DELETE, LFS_TYPE_DIR, LFS_TYPE_FCRC,
    LFS_TYPE_INLINESTRUCT, LFS_TYPE_NAME, LFS_TYPE_SPLICE, LFS_TYPE_STRUCT, LFS_TYPE_TAIL,
};
use crate::tag::{
    lfs_mktag, lfs_tag_chunk, lfs_tag_dsize, lfs_tag_id, lfs_tag_isvalid, lfs_tag_size,
    lfs_tag_splice, lfs_tag_type1, lfs_tag_type2, lfs_tag_type3,
};
use crate::types::{lfs_block_t, lfs_stag_t, lfs_tag_t, LFS_BLOCK_NULL};
use crate::util::{lfs_fromle32, lfs_min, lfs_pair_swap, lfs_scmp, lfs_tole32};
use core::mem;

/// Per lfs.c lfs_dir_fetchmatch (lines 1107-1386)
///
/// C:
/// ```c
/// static lfs_stag_t lfs_dir_fetchmatch(lfs_t *lfs,
///         lfs_mdir_t *dir, const lfs_block_t pair[2],
///         lfs_tag_t fmask, lfs_tag_t ftag, uint16_t *id,
///         int (*cb)(void *data, lfs_tag_t tag, const void *buffer), void *data) {
///     // we can find tag very efficiently during a fetch, since we're already
///     // scanning the entire directory
///     lfs_stag_t besttag = -1;
///
///     // if either block address is invalid we return LFS_ERR_CORRUPT here,
///     // otherwise later writes to the pair could fail
///     if (lfs->block_count
///             && (pair[0] >= lfs->block_count || pair[1] >= lfs->block_count)) {
///         return LFS_ERR_CORRUPT;
///     }
///
///     // find the block with the most recent revision
///     uint32_t revs[2] = {0, 0};
///     int r = 0;
///     for (int i = 0; i < 2; i++) {
///         int err = lfs_bd_read(lfs,
///                 NULL, &lfs->rcache, sizeof(revs[i]),
///                 pair[i], 0, &revs[i], sizeof(revs[i]));
///         revs[i] = lfs_fromle32(revs[i]);
///         if (err && err != LFS_ERR_CORRUPT) {
///             return err;
///         }
///
///         if (err != LFS_ERR_CORRUPT &&
///                 lfs_scmp(revs[i], revs[(i+1)%2]) > 0) {
///             r = i;
///         }
///     }
///
///     dir->pair[0] = pair[(r+0)%2];
///     dir->pair[1] = pair[(r+1)%2];
///     dir->rev = revs[(r+0)%2];
///     dir->off = 0; // nonzero = found some commits
///
///     // now scan tags to fetch the actual dir and find possible match
///     for (int i = 0; i < 2; i++) {
///         lfs_off_t off = 0;
///         lfs_tag_t ptag = 0xffffffff;
///
///         uint16_t tempcount = 0;
///         lfs_block_t temptail[2] = {LFS_BLOCK_NULL, LFS_BLOCK_NULL};
///         bool tempsplit = false;
///         lfs_stag_t tempbesttag = besttag;
///
///         // assume not erased until proven otherwise
///         bool maybeerased = false;
///         bool hasfcrc = false;
///         struct lfs_fcrc fcrc;
///
///         dir->rev = lfs_tole32(dir->rev);
///         uint32_t crc = lfs_crc(0xffffffff, &dir->rev, sizeof(dir->rev));
///         dir->rev = lfs_fromle32(dir->rev);
///
///         while (true) {
///             // extract next tag
///             lfs_tag_t tag;
///             off += lfs_tag_dsize(ptag);
///             int err = lfs_bd_read(lfs,
///                     NULL, &lfs->rcache, lfs->cfg->block_size,
///                     dir->pair[0], off, &tag, sizeof(tag));
///             if (err) {
///                 if (err == LFS_ERR_CORRUPT) {
///                     // can't continue?
///                     break;
///                 }
///                 return err;
///             }
///
///             crc = lfs_crc(crc, &tag, sizeof(tag));
///             tag = lfs_frombe32(tag) ^ ptag;
///
///             // next commit not yet programmed?
///             if (!lfs_tag_isvalid(tag)) {
///                 // we only might be erased if the last tag was a crc
///                 maybeerased = (lfs_tag_type2(ptag) == LFS_TYPE_CCRC);
///                 break;
///             // out of range?
///             } else if (off + lfs_tag_dsize(tag) > lfs->cfg->block_size) {
///                 break;
///             }
///
///             ptag = tag;
///
///             if (lfs_tag_type2(tag) == LFS_TYPE_CCRC) {
///                 // check the crc attr
///                 uint32_t dcrc;
///                 err = lfs_bd_read(lfs,
///                         NULL, &lfs->rcache, lfs->cfg->block_size,
///                         dir->pair[0], off+sizeof(tag), &dcrc, sizeof(dcrc));
///                 if (err) {
///                     if (err == LFS_ERR_CORRUPT) {
///                         break;
///                     }
///                     return err;
///                 }
///                 dcrc = lfs_fromle32(dcrc);
///
///                 if (crc != dcrc) {
///                     break;
///                 }
///
///                 // reset the next bit if we need to
///                 ptag ^= (lfs_tag_t)(lfs_tag_chunk(tag) & 1U) << 31;
///
///                 // toss our crc into the filesystem seed for
///                 // pseudorandom numbers, note we use another crc here
///                 // as a collection function because it is sufficiently
///                 // random and convenient
///                 lfs->seed = lfs_crc(lfs->seed, &crc, sizeof(crc));
///
///                 // update with what's found so far
///                 besttag = tempbesttag;
///                 dir->off = off + lfs_tag_dsize(tag);
///                 dir->etag = ptag;
///                 dir->count = tempcount;
///                 dir->tail[0] = temptail[0];
///                 dir->tail[1] = temptail[1];
///                 dir->split = tempsplit;
///
///                 // reset crc, hasfcrc
///                 crc = 0xffffffff;
///                 continue;
///             }
///
///             // crc the entry first, hopefully leaving it in the cache
///             err = lfs_bd_crc(lfs,
///                     NULL, &lfs->rcache, lfs->cfg->block_size,
///                     dir->pair[0], off+sizeof(tag),
///                     lfs_tag_dsize(tag)-sizeof(tag), &crc);
///             if (err) {
///                 if (err == LFS_ERR_CORRUPT) {
///                     break;
///                 }
///                 return err;
///             }
///
///             // directory modification tags?
///             if (lfs_tag_type1(tag) == LFS_TYPE_NAME) {
///                 // increase count of files if necessary
///                 if (lfs_tag_id(tag) >= tempcount) {
///                     tempcount = lfs_tag_id(tag) + 1;
///                 }
///             } else if (lfs_tag_type1(tag) == LFS_TYPE_SPLICE) {
///                 tempcount += lfs_tag_splice(tag);
///
///                 if (tag == (LFS_MKTAG(LFS_TYPE_DELETE, 0, 0) |
///                         (LFS_MKTAG(0, 0x3ff, 0) & tempbesttag))) {
///                     tempbesttag |= 0x80000000;
///                 } else if (tempbesttag != -1 &&
///                         lfs_tag_id(tag) <= lfs_tag_id(tempbesttag)) {
///                     tempbesttag += LFS_MKTAG(0, lfs_tag_splice(tag), 0);
///                 }
///             } else if (lfs_tag_type1(tag) == LFS_TYPE_TAIL) {
///                 tempsplit = (lfs_tag_chunk(tag) & 1);
///
///                 err = lfs_bd_read(lfs,
///                         NULL, &lfs->rcache, lfs->cfg->block_size,
///                         dir->pair[0], off+sizeof(tag), &temptail, 8);
///                 if (err) {
///                     if (err == LFS_ERR_CORRUPT) {
///                         break;
///                     }
///                     return err;
///                 }
///                 lfs_pair_fromle32(temptail);
///             } else if (lfs_tag_type3(tag) == LFS_TYPE_FCRC) {
///                 err = lfs_bd_read(lfs,
///                         NULL, &lfs->rcache, lfs->cfg->block_size,
///                         dir->pair[0], off+sizeof(tag),
///                         &fcrc, sizeof(fcrc));
///                 if (err) {
///                     if (err == LFS_ERR_CORRUPT) {
///                         break;
///                     }
///                     return err;
///                 }
///
///                 lfs_fcrc_fromle32(&fcrc);
///                 hasfcrc = true;
///             }
///
///             // found a match for our fetcher?
///             if ((fmask & tag) == (fmask & ftag)) {
///                 int res = cb(data, tag, &(struct lfs_diskoff){
///                         dir->pair[0], off+sizeof(tag)});
///                 if (res < 0) {
///                     if (res == LFS_ERR_CORRUPT) {
///                         break;
///                     }
///                     return res;
///                 }
///
///                 if (res == LFS_CMP_EQ) {
///                     // found a match
///                     tempbesttag = tag;
///                 } else if ((LFS_MKTAG(0x7ff, 0x3ff, 0) & tag) ==
///                         (LFS_MKTAG(0x7ff, 0x3ff, 0) & tempbesttag)) {
///                     // found an identical tag, but contents didn't match
///                     // this must mean that our besttag has been overwritten
///                     tempbesttag = -1;
///                 } else if (res == LFS_CMP_GT &&
///                         lfs_tag_id(tag) <= lfs_tag_id(tempbesttag)) {
///                     // found a greater match, keep track to keep things sorted
///                     tempbesttag = tag | 0x80000000;
///                 }
///             }
///         }
///
///         // found no valid commits?
///         if (dir->off == 0) {
///             // try the other block?
///             lfs_pair_swap(dir->pair);
///             dir->rev = revs[(r+1)%2];
///             continue;
///         }
///
///         // did we end on a valid commit? we may have an erased block
///         dir->erased = false;
///         if (maybeerased && dir->off % lfs->cfg->prog_size == 0) {
///         #ifdef LFS_MULTIVERSION
///             // note versions < lfs2.1 did not have fcrc tags, if
///             // we're < lfs2.1 treat missing fcrc as erased data
///             //
///             // we don't strictly need to do this, but otherwise writing
///             // to lfs2.0 disks becomes very inefficient
///             if (lfs_fs_disk_version(lfs) < 0x00020001) {
///                 dir->erased = true;
///
///             } else
///         #endif
///             if (hasfcrc) {
///                 // check for an fcrc matching the next prog's erased state, if
///                 // this failed most likely a previous prog was interrupted, we
///                 // need a new erase
///                 uint32_t fcrc_ = 0xffffffff;
///                 int err = lfs_bd_crc(lfs,
///                         NULL, &lfs->rcache, lfs->cfg->block_size,
///                         dir->pair[0], dir->off, fcrc.size, &fcrc_);
///                 if (err && err != LFS_ERR_CORRUPT) {
///                     return err;
///                 }
///
///                 // found beginning of erased part?
///                 dir->erased = (fcrc_ == fcrc.crc);
///             }
///         }
///
///         // synthetic move
///         if (lfs_gstate_hasmovehere(&lfs->gdisk, dir->pair)) {
///             if (lfs_tag_id(lfs->gdisk.tag) == lfs_tag_id(besttag)) {
///                 besttag |= 0x80000000;
///             } else if (besttag != -1 &&
///                     lfs_tag_id(lfs->gdisk.tag) < lfs_tag_id(besttag)) {
///                 besttag -= LFS_MKTAG(0, 1, 0);
///             }
///         }
///
///         // found tag? or found best id?
///         if (id) {
///             *id = lfs_min(lfs_tag_id(besttag), dir->count);
///         }
///
///         if (lfs_tag_isvalid(besttag)) {
///             return besttag;
///         } else if (lfs_tag_id(besttag) < dir->count) {
///             return LFS_ERR_NOENT;
///         } else {
///             return 0;
///         }
///     }
///
///     LFS_ERROR("Corrupted dir pair at {0x%"PRIx32", 0x%"PRIx32"}",
///             dir->pair[0], dir->pair[1]);
///     return LFS_ERR_CORRUPT;
/// }
///
/// ```
pub fn lfs_dir_fetchmatch(
    _lfs: *const core::ffi::c_void,
    _dir: *mut LfsMdir,
    _pair: *const [lfs_block_t; 2],
    _fmask: lfs_tag_t,
    _ftag: lfs_tag_t,
    _id: *mut u16,
    _cb: Option<
        unsafe extern "C" fn(*mut core::ffi::c_void, lfs_tag_t, *const core::ffi::c_void) -> i32,
    >,
    _data: *mut core::ffi::c_void,
) -> lfs_stag_t {
    // Per lfs.c enum: LFS_CMP_EQ=0, LFS_CMP_LT=1, LFS_CMP_GT=2
    const LFS_CMP_EQ: i32 = 0;
    const LFS_CMP_LT: i32 = 1;
    const LFS_CMP_GT: i32 = 2;

    unsafe {
        let lfs = &mut *(_lfs as *mut crate::fs::Lfs);
        let dir = &mut *_dir;
        let pair = &*_pair;
        let cfg = &*lfs.cfg;

        let mut besttag: lfs_stag_t = -1;
        crate::lfs_trace!("fetchmatch: start pair={:?}", pair);

        // block_count check (C: lines 1117-1120)
        if lfs.block_count != 0 && (pair[0] >= lfs.block_count || pair[1] >= lfs.block_count) {
            return crate::lfs_err!(LFS_ERR_CORRUPT as lfs_stag_t);
        }

        // find the block with the most recent revision (C: lines 1123-1138)
        let mut revs = [0u32; 2];
        let mut r = 0usize;
        for i in 0..2 {
            crate::lfs_trace!("fetchmatch: reading rev for pair[{}]={}", i, pair[i]);
            let mut rev_buf = [0u8; 4];
            let err = lfs_bd_read(
                lfs,
                core::ptr::null(),
                &mut lfs.rcache,
                4,
                pair[i],
                0,
                rev_buf.as_mut_ptr(),
                4,
            );
            revs[i] = u32::from_le_bytes(rev_buf);
            if err != 0 && err != LFS_ERR_CORRUPT {
                return err as lfs_stag_t;
            }
            if err != LFS_ERR_CORRUPT && lfs_scmp(revs[i], revs[(i + 1) % 2]) > 0 {
                r = i;
            }
        }

        dir.pair[0] = pair[r % 2];
        dir.pair[1] = pair[(r + 1) % 2];
        dir.rev = revs[r % 2];
        dir.off = 0;

        for _block_iter in 0..2 {
            crate::lfs_trace!("fetchmatch: block_iter={}", _block_iter);
            let mut off: u32 = 0;
            let mut ptag: lfs_tag_t = 0xffff_ffff;

            let mut tempcount: u16 = 0;
            let mut temptail: [lfs_block_t; 2] = [LFS_BLOCK_NULL, LFS_BLOCK_NULL];
            let mut tempsplit = false;
            let mut tempbesttag = besttag;

            let mut maybeerased = false;
            let mut hasfcrc = false;
            let mut fcrc = LfsFcrc { size: 0, crc: 0 };

            let rev_le = lfs_tole32(dir.rev);
            let mut crc = lfs_crc(0xffff_ffff, &rev_le as *const _ as *const u8, 4);
            dir.rev = lfs_fromle32(dir.rev);

            #[cfg(feature = "loop_limits")]
            let mut tag_iter: u32 = 0;
            // Each tag occupies at least 4 bytes and the walk stops at the end
            // of the block, so a block holds at most block_size/4 tags. Appended
            // commits (each with its own fcrc and ccrc) can fill a block with
            // far more than a fixed small cap allows.
            #[cfg(feature = "loop_limits")]
            let max_fetch_tag_iter: u32 = cfg.block_size / 4 + 1;
            loop {
                #[cfg(feature = "loop_limits")]
                {
                    if tag_iter >= max_fetch_tag_iter {
                        panic!(
                            "loop_limits: max_fetch_tag_iter ({}) exceeded",
                            max_fetch_tag_iter
                        );
                    }
                    if tag_iter > 0 && tag_iter.is_multiple_of(32) {
                        crate::lfs_trace!(
                            "fetchmatch: tag_iter={} off={} block_iter={} pair={:?}",
                            tag_iter,
                            off,
                            _block_iter,
                            dir.pair
                        );
                    }
                    tag_iter += 1;
                }

                off += lfs_tag_dsize(ptag);

                let mut tag_buf = [0u8; 4];
                let err = lfs_bd_read(
                    lfs,
                    core::ptr::null(),
                    &mut lfs.rcache,
                    cfg.block_size,
                    dir.pair[0],
                    off,
                    tag_buf.as_mut_ptr(),
                    4,
                );
                if err != 0 {
                    if err == LFS_ERR_CORRUPT {
                        break;
                    }
                    return err as lfs_stag_t;
                }

                crc = lfs_crc(crc, tag_buf.as_ptr(), 4);
                let tag_raw = u32::from_be_bytes(tag_buf);
                let mut tag = tag_raw ^ ptag;

                if !lfs_tag_isvalid(tag) {
                    maybeerased = u32::from(lfs_tag_type2(ptag)) == LFS_TYPE_CCRC;
                    break;
                } else if off + lfs_tag_dsize(tag) > cfg.block_size {
                    break;
                }

                ptag = tag;

                if u32::from(lfs_tag_type2(tag)) == LFS_TYPE_CCRC {
                    let mut dcrc_buf = [0u8; 4];
                    let err = lfs_bd_read(
                        lfs,
                        core::ptr::null(),
                        &mut lfs.rcache,
                        cfg.block_size,
                        dir.pair[0],
                        off + 4,
                        dcrc_buf.as_mut_ptr(),
                        4,
                    );
                    if err != 0 {
                        if err == LFS_ERR_CORRUPT {
                            break;
                        }
                        return err as lfs_stag_t;
                    }
                    let dcrc = u32::from_le_bytes(dcrc_buf);

                    if crc != dcrc {
                        break;
                    }

                    ptag ^= (lfs_tag_chunk(tag) as lfs_tag_t & 1) << 31;

                    lfs.seed = lfs_crc(lfs.seed, &crc as *const _ as *const u8, 4);

                    besttag = tempbesttag;
                    dir.off = off + lfs_tag_dsize(tag);
                    dir.etag = ptag;
                    dir.count = tempcount;
                    dir.tail[0] = temptail[0];
                    dir.tail[1] = temptail[1];
                    dir.split = tempsplit;

                    crc = 0xffff_ffff;
                    continue;
                }

                let entry_size = lfs_tag_dsize(tag) - 4;
                let mut crc_val = crc;
                let err = lfs_bd_crc(
                    lfs,
                    core::ptr::null(),
                    &mut lfs.rcache,
                    cfg.block_size,
                    dir.pair[0],
                    off + 4,
                    entry_size,
                    &mut crc_val,
                );
                if err != 0 {
                    if err == LFS_ERR_CORRUPT {
                        break;
                    }
                    return err as lfs_stag_t;
                }
                crc = crc_val;

                if u32::from(lfs_tag_type1(tag)) == LFS_TYPE_NAME {
                    if lfs_tag_id(tag) >= tempcount {
                        tempcount = lfs_tag_id(tag) + 1;
                    }
                } else if u32::from(lfs_tag_type1(tag)) == LFS_TYPE_SPLICE {
                    // Divergence: C uses tempcount += lfs_tag_splice(tag) (unsigned wrap). We clamp
                    // to 0 to avoid underflow when splice is negative (Rule 7).
                    let delta = lfs_tag_splice(tag) as i32;
                    tempcount = (tempcount as i32 + delta).max(0) as u16;

                    let delete_tag = lfs_mktag(LFS_TYPE_DELETE, 0, 0)
                        | (lfs_mktag(0, 0x3ff, 0) & tempbesttag as lfs_tag_t);
                    if tag == delete_tag {
                        tempbesttag = (tempbesttag as u32 | 0x8000_0000) as lfs_stag_t;
                    } else if tempbesttag != -1
                        && lfs_tag_id(tag) <= lfs_tag_id(tempbesttag as lfs_tag_t)
                    {
                        // C adds the (possibly negative) splice with unsigned wrap.
                        tempbesttag = (tempbesttag as lfs_tag_t).wrapping_add(lfs_mktag(
                            0,
                            lfs_tag_splice(tag) as u32,
                            0,
                        )) as lfs_stag_t;
                    }
                } else if u32::from(lfs_tag_type1(tag)) == LFS_TYPE_TAIL {
                    tempsplit = (lfs_tag_chunk(tag) & 1) != 0;

                    let mut tail_buf = [0u8; 8];
                    let err = lfs_bd_read(
                        lfs,
                        core::ptr::null(),
                        &mut lfs.rcache,
                        cfg.block_size,
                        dir.pair[0],
                        off + 4,
                        tail_buf.as_mut_ptr(),
                        8,
                    );
                    if err != 0 {
                        if err == LFS_ERR_CORRUPT {
                            break;
                        }
                        return err as lfs_stag_t;
                    }
                    temptail[0] = u32::from_le_bytes(tail_buf[0..4].try_into().unwrap());
                    temptail[1] = u32::from_le_bytes(tail_buf[4..8].try_into().unwrap());
                } else if u32::from(lfs_tag_type3(tag)) == LFS_TYPE_FCRC {
                    let mut fcrc_buf = [0u8; mem::size_of::<LfsFcrc>()];
                    let err = lfs_bd_read(
                        lfs,
                        core::ptr::null(),
                        &mut lfs.rcache,
                        cfg.block_size,
                        dir.pair[0],
                        off + 4,
                        fcrc_buf.as_mut_ptr(),
                        fcrc_buf.len() as u32,
                    );
                    if err != 0 {
                        if err == LFS_ERR_CORRUPT {
                            break;
                        }
                        return err as lfs_stag_t;
                    }
                    core::ptr::copy_nonoverlapping(
                        fcrc_buf.as_ptr(),
                        &mut fcrc as *mut LfsFcrc as *mut u8,
                        mem::size_of::<LfsFcrc>(),
                    );
                    lfs_fcrc_fromle32(&mut fcrc);
                    hasfcrc = true;
                }

                if (_fmask & tag) == (_fmask & _ftag) {
                    if let Some(cb) = _cb {
                        let diskoff = crate::tag::lfs_diskoff {
                            block: dir.pair[0],
                            off: off + 4,
                        };
                        let res = cb(_data, tag, &diskoff as *const _ as *const core::ffi::c_void);
                        if res < 0 {
                            if res == LFS_ERR_CORRUPT {
                                break;
                            }
                            return res as lfs_stag_t;
                        }

                        if res == LFS_CMP_EQ {
                            tempbesttag = tag as lfs_stag_t;
                        } else if (lfs_mktag(0x7ff, 0x3ff, 0) & tag)
                            == (lfs_mktag(0x7ff, 0x3ff, 0) & tempbesttag as lfs_tag_t)
                        {
                            tempbesttag = -1;
                        } else if res == LFS_CMP_GT
                            && lfs_tag_id(tag) <= lfs_tag_id(tempbesttag as lfs_tag_t)
                        {
                            tempbesttag = (tag | 0x8000_0000) as lfs_stag_t;
                        }
                    }
                }
            }

            if dir.off == 0 {
                lfs_pair_swap(&mut dir.pair);
                dir.rev = revs[(r + 1) % 2];
                continue;
            }

            dir.erased = false;
            if maybeerased && dir.off.is_multiple_of(cfg.prog_size) && hasfcrc {
                let mut fcrc_ = 0xffff_ffffu32;
                let err = lfs_bd_crc(
                    lfs,
                    core::ptr::null(),
                    &mut lfs.rcache,
                    cfg.block_size,
                    dir.pair[0],
                    dir.off,
                    fcrc.size,
                    &mut fcrc_,
                );
                if err != 0 && err != LFS_ERR_CORRUPT {
                    return err as lfs_stag_t;
                }
                dir.erased = fcrc_ == fcrc.crc;
            }

            if lfs_gstate_hasmovehere(&lfs.gdisk, &dir.pair) {
                if lfs_tag_id(lfs.gdisk.tag) == lfs_tag_id(besttag as lfs_tag_t) {
                    besttag = (besttag as u32 | 0x8000_0000) as lfs_stag_t;
                } else if besttag != -1
                    && lfs_tag_id(lfs.gdisk.tag) < lfs_tag_id(besttag as lfs_tag_t)
                {
                    besttag -= lfs_mktag(0, 1, 0) as lfs_stag_t;
                }
            }

            if !_id.is_null() {
                *_id = lfs_min(lfs_tag_id(besttag as lfs_tag_t) as u32, dir.count as u32) as u16;
            }

            if lfs_tag_isvalid(besttag as lfs_tag_t) {
                crate::lfs_trace!(
                    "fetchmatch: FOUND besttag=0x{:08x} pair=[{},{}] count={} split={} tail=[{},{}]",
                    besttag as u32,
                    dir.pair[0],
                    dir.pair[1],
                    dir.count,
                    dir.split,
                    dir.tail[0],
                    dir.tail[1]
                );
                return besttag;
            } else if lfs_tag_id(besttag as lfs_tag_t) < dir.count {
                crate::lfs_trace!(
                    "fetchmatch: NOENT pair=[{},{}] count={} besttag_id={} split={} tail=[{},{}]",
                    dir.pair[0],
                    dir.pair[1],
                    dir.count,
                    lfs_tag_id(besttag as lfs_tag_t),
                    dir.split,
                    dir.tail[0],
                    dir.tail[1]
                );
                return crate::error::LFS_ERR_NOENT as lfs_stag_t;
            } else {
                crate::lfs_trace!(
                    "fetchmatch: CONTINUE pair=[{},{}] count={} split={} tail=[{},{}]",
                    dir.pair[0],
                    dir.pair[1],
                    dir.count,
                    dir.split,
                    dir.tail[0],
                    dir.tail[1]
                );
                return 0;
            }
        }

        LFS_ERR_CORRUPT as lfs_stag_t
    }
}

/// Per lfs.c lfs_dir_fetch (lines 1387-1393)
///
/// C:
/// ```c
/// static int lfs_dir_fetch(lfs_t *lfs,
///         lfs_mdir_t *dir, const lfs_block_t pair[2]) {
///     // note, mask=-1, tag=-1 can never match a tag since this
///     // pattern has the invalid bit set
///     return (int)lfs_dir_fetchmatch(lfs, dir, pair,
///             (lfs_tag_t)-1, (lfs_tag_t)-1, NULL, NULL, NULL);
/// }
/// ```
pub fn lfs_dir_fetch(lfs: *mut crate::fs::Lfs, dir: *mut LfsMdir, pair: &[lfs_block_t; 2]) -> i32 {
    let res = lfs_dir_fetchmatch(
        lfs as *mut _ as *const core::ffi::c_void,
        dir,
        pair as *const _,
        0xffff_ffff,
        0xffff_ffff,
        core::ptr::null_mut(),
        None,
        core::ptr::null_mut(),
    );
    if res < 0 {
        res
    } else {
        0
    }
}

/// Per lfs.c lfs_dir_getgstate (lines 1395-1411)
///
/// C:
/// ```c
/// static int lfs_dir_getgstate(lfs_t *lfs, const lfs_mdir_t *dir,
///         lfs_gstate_t *gstate) {
///     lfs_gstate_t temp;
///     lfs_stag_t res = lfs_dir_get(lfs, dir, LFS_MKTAG(0x7ff, 0, 0),
///             LFS_MKTAG(LFS_TYPE_MOVESTATE, 0, sizeof(temp)), &temp);
///     if (res < 0 && res != LFS_ERR_NOENT) {
///         return res;
///     }
///
///     if (res != LFS_ERR_NOENT) {
///         // xor together to find resulting gstate
///         lfs_gstate_fromle32(&temp);
///         lfs_gstate_xor(gstate, &temp);
///     }
///
///     return 0;
/// }
/// ```
pub fn lfs_dir_getgstate(
    lfs: *mut crate::fs::Lfs,
    dir: *const LfsMdir,
    gstate: *mut LfsGstate,
) -> i32 {
    unsafe {
        let mut temp = crate::lfs_gstate::LfsGstate {
            tag: 0,
            pair: [0, 0],
        };
        let res = lfs_dir_get(
            lfs,
            dir,
            crate::tag::lfs_mktag(0x7ff, 0, 0),
            crate::tag::lfs_mktag(
                crate::lfs_type::lfs_type::LFS_TYPE_MOVESTATE,
                0,
                core::mem::size_of::<LfsGstate>() as u32,
            ),
            &mut temp as *mut _ as *mut core::ffi::c_void,
        );
        if res < 0 && res != crate::error::LFS_ERR_NOENT {
            return res;
        }
        if res != crate::error::LFS_ERR_NOENT {
            lfs_gstate_fromle32(&mut temp);
            lfs_gstate_xor(&mut *gstate, &temp);
        }
        0
    }
}

/// Per lfs.c lfs_dir_getinfo (lines 1413-1445)
///
/// C:
/// ```c
/// static int lfs_dir_getinfo(lfs_t *lfs, lfs_mdir_t *dir,
///         uint16_t id, struct lfs_info *info) {
///     if (id == 0x3ff) {
///         // special case for root
///         strcpy(info->name, "/");
///         info->type = LFS_TYPE_DIR;
///         return 0;
///     }
///
///     lfs_stag_t tag = lfs_dir_get(lfs, dir, LFS_MKTAG(0x780, 0x3ff, 0),
///             LFS_MKTAG(LFS_TYPE_NAME, id, lfs->name_max+1), info->name);
///     if (tag < 0) {
///         return (int)tag;
///     }
///
///     info->type = lfs_tag_type3(tag);
///
///     struct lfs_ctz ctz;
///     tag = lfs_dir_get(lfs, dir, LFS_MKTAG(0x700, 0x3ff, 0),
///             LFS_MKTAG(LFS_TYPE_STRUCT, id, sizeof(ctz)), &ctz);
///     if (tag < 0) {
///         return (int)tag;
///     }
///     lfs_ctz_fromle32(&ctz);
///
///     if (lfs_tag_type3(tag) == LFS_TYPE_CTZSTRUCT) {
///         info->size = ctz.size;
///     } else if (lfs_tag_type3(tag) == LFS_TYPE_INLINESTRUCT) {
///         info->size = lfs_tag_size(tag);
///     }
///
///     return 0;
/// }
/// ```
pub fn lfs_dir_getinfo(
    lfs: *mut crate::fs::Lfs,
    dir: *const LfsMdir,
    id: u16,
    info: *mut LfsInfo,
) -> i32 {
    if lfs.is_null() || dir.is_null() || info.is_null() {
        return crate::error::LFS_ERR_INVAL;
    }
    unsafe {
        let info = &mut *info;
        let lfs = &*lfs;
        let dir_ref = &*dir;

        // C: lfs.c:1415-1420
        if id == 0x3ff {
            info.type_ = LFS_TYPE_DIR as u8;
            info.name[0] = b'/';
            info.name[1] = 0;
            return 0;
        }

        // C: lfs.c:1422-1426
        let name_max = lfs.name_max;
        let tag = lfs_dir_get(
            lfs as *const _ as *mut _,
            dir,
            lfs_mktag(0x780, 0x3ff, 0),
            lfs_mktag(LFS_TYPE_NAME, id as u32, name_max + 1),
            info.name.as_mut_ptr() as *mut core::ffi::c_void,
        );
        if tag < 0 {
            return tag;
        }

        info.type_ = lfs_tag_type3(tag as u32) as u8;

        // C: lfs.c:1430-1441
        let mut ctz = LfsCtz { head: 0, size: 0 };
        let tag = lfs_dir_get(
            lfs as *const _ as *mut _,
            dir,
            lfs_mktag(0x700, 0x3ff, 0),
            lfs_mktag(LFS_TYPE_STRUCT, id as u32, mem::size_of::<LfsCtz>() as u32),
            &mut ctz as *mut _ as *mut core::ffi::c_void,
        );
        if tag < 0 {
            return tag;
        }
        lfs_ctz_fromle32(&mut ctz);

        if u32::from(lfs_tag_type3(tag as u32)) == LFS_TYPE_CTZSTRUCT {
            info.size = ctz.size;
        } else if u32::from(lfs_tag_type3(tag as u32)) == LFS_TYPE_INLINESTRUCT {
            info.size = lfs_tag_size(tag as u32);
        }

        0
    }
}
