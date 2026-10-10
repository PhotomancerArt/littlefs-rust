//! Directory traverse. Per lfs.c lfs_dir_traverse, lfs_dir_getslice, lfs_dir_get, lfs_dir_getread.

use crate::bd::LfsCache;
use crate::dir::LfsMdir;
use crate::types::{lfs_block_t, lfs_off_t, lfs_size_t, lfs_stag_t, lfs_tag_t};

/// Per lfs.c lfs_dir_getslice (lines 719-784)
///
/// C:
/// ```c
/// static lfs_stag_t lfs_dir_getslice(lfs_t *lfs, const lfs_mdir_t *dir,
///         lfs_tag_t gmask, lfs_tag_t gtag,
///         lfs_off_t goff, void *gbuffer, lfs_size_t gsize) {
///     lfs_off_t off = dir->off;
///     lfs_tag_t ntag = dir->etag;
///     lfs_stag_t gdiff = 0;
///
///     // synthetic moves
///     if (lfs_gstate_hasmovehere(&lfs->gdisk, dir->pair) &&
///             lfs_tag_id(gmask) != 0) {
///         if (lfs_tag_id(lfs->gdisk.tag) == lfs_tag_id(gtag)) {
///             return LFS_ERR_NOENT;
///         } else if (lfs_tag_id(lfs->gdisk.tag) < lfs_tag_id(gtag)) {
///             gdiff -= LFS_MKTAG(0, 1, 0);
///         }
///     }
///
///     // iterate over dir block backwards (for faster lookups)
///     while (off >= sizeof(lfs_tag_t) + lfs_tag_dsize(ntag)) {
///         off -= lfs_tag_dsize(ntag);
///         lfs_tag_t tag = ntag;
///         int err = lfs_bd_read(lfs,
///                 NULL, &lfs->rcache, sizeof(ntag),
///                 dir->pair[0], off, &ntag, sizeof(ntag));
///         LFS_ASSERT(err <= 0);
///         if (err) {
///             return err;
///         }
///
///         ntag = (lfs_frombe32(ntag) ^ tag) & 0x7fffffff;
///
///         if (lfs_tag_id(gmask) != 0 &&
///                 lfs_tag_type1(tag) == LFS_TYPE_SPLICE &&
///                 lfs_tag_id(tag) <= lfs_tag_id(gtag - gdiff)) {
///             if (tag == (LFS_MKTAG(LFS_TYPE_CREATE, 0, 0) |
///                     (LFS_MKTAG(0, 0x3ff, 0) & (gtag - gdiff)))) {
///                 // found where we were created
///                 return LFS_ERR_NOENT;
///             }
///
///             // move around splices
///             gdiff += LFS_MKTAG(0, lfs_tag_splice(tag), 0);
///         }
///
///         if ((gmask & tag) == (gmask & (gtag - gdiff))) {
///             if (lfs_tag_isdelete(tag)) {
///                 return LFS_ERR_NOENT;
///             }
///
///             lfs_size_t diff = lfs_min(lfs_tag_size(tag), gsize);
///             err = lfs_bd_read(lfs,
///                     NULL, &lfs->rcache, diff,
///                     dir->pair[0], off+sizeof(tag)+goff, gbuffer, diff);
///             LFS_ASSERT(err <= 0);
///             if (err) {
///                 return err;
///             }
///
///             memset((uint8_t*)gbuffer + diff, 0, gsize - diff);
///
///             return tag + gdiff;
///         }
///     }
///
///     return LFS_ERR_NOENT;
/// }
/// ```
pub fn lfs_dir_getslice(
    lfs: *mut crate::fs::Lfs,
    dir: *const LfsMdir,
    gmask: lfs_tag_t,
    gtag: lfs_tag_t,
    goff: lfs_off_t,
    gbuffer: *mut core::ffi::c_void,
    gsize: lfs_size_t,
) -> lfs_stag_t {
    use crate::bd::bd::lfs_bd_read;
    use crate::tag::{
        lfs_mktag, lfs_tag_dsize, lfs_tag_id, lfs_tag_isdelete, lfs_tag_size, lfs_tag_type1,
    };
    use crate::util::{lfs_frombe32, lfs_min};

    unsafe {
        let dir_ref = &*dir;
        let mut off = dir_ref.off;
        let mut ntag = dir_ref.etag;
        let mut gdiff: lfs_stag_t = 0;

        if crate::lfs_gstate::lfs_gstate_hasmovehere(&(*lfs).gdisk, &dir_ref.pair)
            && lfs_tag_id(gmask) != 0
        {
            if lfs_tag_id((*lfs).gdisk.tag) == lfs_tag_id(gtag) {
                return crate::error::LFS_ERR_NOENT;
            } else if lfs_tag_id((*lfs).gdisk.tag) < lfs_tag_id(gtag) {
                gdiff = gdiff.wrapping_sub(lfs_mktag(0, 1, 0) as i32);
            }
        }

        // Each step moves back at least one 4-byte tag, so the walk is bounded
        // by the committed size of the block.
        #[cfg(feature = "loop_limits")]
        let max_getslice_tag_iter: u32 = dir_ref.off / 4 + 1;
        #[cfg(feature = "loop_limits")]
        let mut tag_iter: u32 = 0;
        while off >= 4 + lfs_tag_dsize(ntag) {
            #[cfg(feature = "loop_limits")]
            {
                if tag_iter >= max_getslice_tag_iter {
                    panic!(
                        "loop_limits: max_getslice_tag_iter ({}) exceeded",
                        max_getslice_tag_iter
                    );
                }
                tag_iter += 1;
            }
            off -= lfs_tag_dsize(ntag);
            let tag = ntag;
            let mut ntag_buf: lfs_tag_t = 0;
            let err = lfs_bd_read(
                lfs,
                core::ptr::null_mut(),
                unsafe { &mut (*lfs).rcache },
                4,
                dir_ref.pair[0],
                off,
                &mut ntag_buf as *mut _ as *mut u8,
                4,
            );
            if err != 0 {
                return err as lfs_stag_t;
            }
            ntag = (lfs_frombe32(ntag_buf) ^ tag) & 0x7fff_ffff;

            if lfs_tag_id(gmask) != 0
                && u32::from(lfs_tag_type1(tag)) == crate::lfs_type::lfs_type::LFS_TYPE_SPLICE
                && lfs_tag_id(tag) <= lfs_tag_id((gtag as i32 - gdiff) as u32)
            {
                if tag
                    == (lfs_mktag(crate::lfs_type::lfs_type::LFS_TYPE_CREATE, 0, 0)
                        | (lfs_mktag(0, 0x3ff, 0) & (gtag as i32 - gdiff) as u32))
                {
                    return crate::error::LFS_ERR_NOENT;
                }
                gdiff = gdiff
                    .wrapping_add(lfs_mktag(0, crate::tag::lfs_tag_splice(tag) as u32, 0) as i32);
            }

            if (gmask & tag) == (gmask & ((gtag as i32 - gdiff) as u32)) {
                if lfs_tag_isdelete(tag) {
                    return crate::error::LFS_ERR_NOENT;
                }
                let diff = lfs_min(lfs_tag_size(tag), gsize);
                let err = lfs_bd_read(
                    lfs,
                    core::ptr::null_mut(),
                    unsafe { &mut (*lfs).rcache },
                    diff,
                    dir_ref.pair[0],
                    off + 4 + goff,
                    gbuffer as *mut u8,
                    diff,
                );
                if err != 0 {
                    return err as lfs_stag_t;
                }
                if !gbuffer.is_null() && diff < gsize {
                    core::ptr::write_bytes(
                        (gbuffer as *mut u8).add(diff as usize),
                        0,
                        (gsize - diff) as usize,
                    );
                }
                return (tag as i32).wrapping_add(gdiff);
            }
        }
        crate::error::LFS_ERR_NOENT
    }
}

/// Per lfs.c lfs_dir_get (lines 786-791)
///
/// C:
/// ```c
/// static lfs_stag_t lfs_dir_get(lfs_t *lfs, const lfs_mdir_t *dir,
///         lfs_tag_t gmask, lfs_tag_t gtag, void *buffer) {
///     return lfs_dir_getslice(lfs, dir,
///             gmask, gtag,
///             0, buffer, lfs_tag_size(gtag));
/// }
/// ```
pub fn lfs_dir_get(
    lfs: *mut crate::fs::Lfs,
    dir: *const LfsMdir,
    gmask: lfs_tag_t,
    gtag: lfs_tag_t,
    buffer: *mut core::ffi::c_void,
) -> lfs_stag_t {
    lfs_dir_getslice(
        lfs,
        dir,
        gmask,
        gtag,
        0,
        buffer,
        crate::tag::lfs_tag_size(gtag),
    )
}

/// Per lfs.c lfs_dir_getread (lines 793-850)
///
/// Translation docs: Read inline file data from directory entry. Uses pcache/rcache
/// to avoid repeated dir reads. Copies bytes from [off, off+size) into buffer.
///
/// C:
/// ```c
/// static int lfs_dir_getread(lfs_t *lfs, const lfs_mdir_t *dir,
///         const lfs_cache_t *pcache, lfs_cache_t *rcache, lfs_size_t hint,
///         lfs_tag_t gmask, lfs_tag_t gtag,
///         lfs_off_t off, void *buffer, lfs_size_t size) {
///     uint8_t *data = buffer;
///     if (off+size > lfs->cfg->block_size) {
///         return LFS_ERR_CORRUPT;
///     }
///
///     while (size > 0) {
///         lfs_size_t diff = size;
///         ... (pcache/rcache checks, then load via getslice)
///     }
///     return 0;
/// }
/// ```
pub fn lfs_dir_getread(
    lfs: *mut crate::fs::Lfs,
    dir: *const LfsMdir,
    pcache: *const LfsCache,
    rcache: *mut LfsCache,
    hint: lfs_size_t,
    gmask: lfs_tag_t,
    gtag: lfs_tag_t,
    off: lfs_off_t,
    buffer: *mut core::ffi::c_void,
    size: lfs_size_t,
) -> i32 {
    use crate::error::LFS_ERR_CORRUPT;
    use crate::types::LFS_BLOCK_INLINE;
    use crate::util::{lfs_aligndown, lfs_alignup, lfs_min};

    if buffer.is_null() {
        return 0;
    }
    let data = buffer as *mut u8;

    unsafe {
        let lfs_ref = &*lfs;
        let cfg = lfs_ref.cfg.as_ref().expect("cfg");
        if off + size > cfg.block_size {
            return crate::lfs_err!(LFS_ERR_CORRUPT);
        }

        let mut off = off;
        let mut size = size;
        let mut data = data;

        while size > 0 {
            let mut diff = size;

            if !pcache.is_null() {
                let pcache_ref = &*pcache;
                if pcache_ref.block == LFS_BLOCK_INLINE && off < pcache_ref.off + pcache_ref.size {
                    if off >= pcache_ref.off {
                        diff = lfs_min(diff, pcache_ref.size - (off - pcache_ref.off));
                        if !pcache_ref.buffer.is_null() {
                            core::ptr::copy_nonoverlapping(
                                pcache_ref.buffer.add((off - pcache_ref.off) as usize),
                                data,
                                diff as usize,
                            );
                        }
                        data = data.add(diff as usize);
                        off += diff;
                        size -= diff;
                        continue;
                    }
                    diff = lfs_min(diff, pcache_ref.off - off);
                }
            }

            let rcache_ref = &mut *rcache;
            if rcache_ref.block == LFS_BLOCK_INLINE
                && off < rcache_ref.off + rcache_ref.size
                && off >= rcache_ref.off
            {
                diff = lfs_min(diff, rcache_ref.size - (off - rcache_ref.off));
                if !rcache_ref.buffer.is_null() {
                    core::ptr::copy_nonoverlapping(
                        rcache_ref.buffer.add((off - rcache_ref.off) as usize),
                        data,
                        diff as usize,
                    );
                }
                data = data.add(diff as usize);
                off += diff;
                size -= diff;
                continue;
            }

            rcache_ref.block = LFS_BLOCK_INLINE;
            rcache_ref.off = lfs_aligndown(off, cfg.read_size);
            rcache_ref.size = lfs_min(lfs_alignup(off + hint, cfg.read_size), cfg.cache_size);
            let res = lfs_dir_getslice(
                lfs,
                dir,
                gmask,
                gtag,
                rcache_ref.off,
                rcache_ref.buffer as *mut core::ffi::c_void,
                rcache_ref.size,
            );
            if res < 0 {
                return res as i32;
            }
        }
    }
    0
}

/// Per lfs.c lfs_dir_traverse_filter (lines 852-910)
///
/// C:
/// ```c
/// static int lfs_dir_traverse_filter(void *p,
///         lfs_tag_t tag, const void *buffer) {
///     lfs_tag_t *filtertag = p;
///     (void)buffer;
///
///     // which mask depends on unique bit in tag structure
///     uint32_t mask = (tag & LFS_MKTAG(0x100, 0, 0))
///             ? LFS_MKTAG(0x7ff, 0x3ff, 0)
///             : LFS_MKTAG(0x700, 0x3ff, 0);
///
///     // check for redundancy
///     if ((mask & tag) == (mask & *filtertag) ||
///             lfs_tag_isdelete(*filtertag) ||
///             (LFS_MKTAG(0x7ff, 0x3ff, 0) & tag) == (
///                 LFS_MKTAG(LFS_TYPE_DELETE, 0, 0) |
///                     (LFS_MKTAG(0, 0x3ff, 0) & *filtertag))) {
///         *filtertag = LFS_MKTAG(LFS_FROM_NOOP, 0, 0);
///         return true;
///     }
///
///     // check if we need to adjust for created/deleted tags
///     if (lfs_tag_type1(tag) == LFS_TYPE_SPLICE &&
///             lfs_tag_id(tag) <= lfs_tag_id(*filtertag)) {
///         *filtertag += LFS_MKTAG(0, lfs_tag_splice(tag), 0);
///     }
///
///     return false;
/// }
/// #endif
///
/// #ifndef LFS_READONLY
/// // maximum recursive depth of lfs_dir_traverse, the deepest call:
/// //
/// // traverse with commit
/// // '-> traverse with move
/// //     '-> traverse with filter
/// //
/// #define LFS_DIR_TRAVERSE_DEPTH 3
///
/// struct lfs_dir_traverse {
///     const lfs_mdir_t *dir;
///     lfs_off_t off;
///     lfs_tag_t ptag;
///     const struct lfs_mattr *attrs;
///     int attrcount;
///
///     lfs_tag_t tmask;
///     lfs_tag_t ttag;
///     uint16_t begin;
///     uint16_t end;
///     int16_t diff;
///
///     int (*cb)(void *data, lfs_tag_t tag, const void *buffer);
///     void *data;
///
///     lfs_tag_t tag;
///     const void *buffer;
///     struct lfs_diskoff disk;
/// };
/// ```
pub unsafe extern "C" fn lfs_dir_traverse_filter(
    p: *mut core::ffi::c_void,
    tag: lfs_tag_t,
    _buffer: *const core::ffi::c_void,
) -> i32 {
    use crate::lfs_type::lfs_type::{LFS_FROM_NOOP, LFS_TYPE_DELETE, LFS_TYPE_SPLICE};
    use crate::tag::{lfs_mktag, lfs_tag_id, lfs_tag_isdelete, lfs_tag_splice, lfs_tag_type1};

    let filtertag = p as *mut lfs_tag_t;
    let ft = unsafe { *filtertag };
    crate::lfs_trace!(
        "traverse_filter: tag=0x{:08x} ft=0x{:08x} mask_check tag&0x100={}",
        tag,
        ft,
        (tag & lfs_mktag(0x100, 0, 0)) != 0
    );

    let mask = if (tag & lfs_mktag(0x100, 0, 0)) != 0 {
        lfs_mktag(0x7ff, 0x3ff, 0)
    } else {
        lfs_mktag(0x700, 0x3ff, 0)
    };

    if (mask & tag) == (mask & ft)
        || lfs_tag_isdelete(ft)
        || (lfs_mktag(0x7ff, 0x3ff, 0) & tag)
            == (lfs_mktag(LFS_TYPE_DELETE, 0, 0) | (lfs_mktag(0, 0x3ff, 0) & ft))
    {
        crate::lfs_trace!(
            "traverse_filter: redundant tag=0x{:08x} ft=0x{:08x} -> NOOP return 1",
            tag,
            ft
        );
        unsafe { *filtertag = lfs_mktag(LFS_FROM_NOOP, 0, 0) };
        return 1;
    }

    if u32::from(lfs_tag_type1(tag)) == LFS_TYPE_SPLICE && lfs_tag_id(tag) <= lfs_tag_id(ft) {
        unsafe {
            *filtertag = ft.wrapping_add(lfs_mktag(0, lfs_tag_splice(tag) as u32, 0));
        }
    }

    0
}

/// Maximum recursive depth. C: LFS_DIR_TRAVERSE_DEPTH 3.
const LFS_DIR_TRAVERSE_DEPTH: usize = 3;

/// Phases for the traverse state machine. Option 3 from traverse-restructure-notes.
enum TraversePhase {
    GetNextTag,
    ProcessTag {
        tag: lfs_tag_t,
        buffer: *const core::ffi::c_void,
        /// When dispatching a tag from disk after exhaust-pop, frame.buffer pointed to
        /// the outer `disk` (overwritten by subsequent reads). Use this copy instead.
        disk_override: Option<crate::tag::lfs_diskoff>,
    },
    PopAndProcess,
}

/// Empty attrs slice for LFS_FROM_MOVE recursion (we traverse source dir from disk only).
const EMPTY_ATTRS: &[crate::tag::lfs_mattr] = &[];

/// Stack frame for lfs_dir_traverse recursion. Per lfs.c struct lfs_dir_traverse.
/// C has .buffer = buffer; we must store it for attr-backed tags (e.g. SUPERBLOCK).
/// When the filter marks redundant (returns 1, sets tag=NOOP), we store the tag we were
/// processing so it still gets committed when we pop.
struct LfsDirTraverseStack {
    dir: *const LfsMdir,
    off: lfs_off_t,
    ptag: lfs_tag_t,
    attr_i: usize,
    use_empty_attrs: bool,
    tmask: lfs_tag_t,
    ttag: lfs_tag_t,
    begin: u16,
    end: u16,
    diff: i16,
    cb: unsafe extern "C" fn(*mut core::ffi::c_void, lfs_tag_t, *const core::ffi::c_void) -> i32,
    data: *mut core::ffi::c_void,
    tag: lfs_tag_t,
    buffer: *const core::ffi::c_void,
    disk: crate::tag::lfs_diskoff,
    /// Tag we were processing when filter returned 1; use when popping with NOOP.
    redundant_tag: lfs_tag_t,
    redundant_buffer: *const core::ffi::c_void,
}

/// Per lfs.c lfs_dir_traverse (lines 912-1105)
///
/// C:
/// ```c
/// static int lfs_dir_traverse(lfs_t *lfs,
///         const lfs_mdir_t *dir, lfs_off_t off, lfs_tag_t ptag,
///         const struct lfs_mattr *attrs, int attrcount,
///         lfs_tag_t tmask, lfs_tag_t ttag,
///         uint16_t begin, uint16_t end, int16_t diff,
///         int (*cb)(void *data, lfs_tag_t tag, const void *buffer), void *data) {
///     // This function in inherently recursive, but bounded. To allow tool-based
///     // analysis without unnecessary code-cost we use an explicit stack
///     struct lfs_dir_traverse stack[LFS_DIR_TRAVERSE_DEPTH-1];
///     unsigned sp = 0;
///     int res;
///
///     // iterate over directory and attrs
///     lfs_tag_t tag;
///     const void *buffer;
///     struct lfs_diskoff disk = {0};
///     while (true) {
///         {
///             if (off+lfs_tag_dsize(ptag) < dir->off) {
///                 off += lfs_tag_dsize(ptag);
///                 int err = lfs_bd_read(lfs,
///                         NULL, &lfs->rcache, sizeof(tag),
///                         dir->pair[0], off, &tag, sizeof(tag));
///                 if (err) {
///                     return err;
///                 }
///
///                 tag = (lfs_frombe32(tag) ^ ptag) | 0x80000000;
///                 disk.block = dir->pair[0];
///                 disk.off = off+sizeof(lfs_tag_t);
///                 buffer = &disk;
///                 ptag = tag;
///             } else if (attrcount > 0) {
///                 tag = attrs[0].tag;
///                 buffer = attrs[0].buffer;
///                 attrs += 1;
///                 attrcount -= 1;
///             } else {
///                 // finished traversal, pop from stack?
///                 res = 0;
///                 break;
///             }
///
///             // do we need to filter?
///             lfs_tag_t mask = LFS_MKTAG(0x7ff, 0, 0);
///             if ((mask & tmask & tag) != (mask & tmask & ttag)) {
///                 continue;
///             }
///
///             if (lfs_tag_id(tmask) != 0) {
///                 LFS_ASSERT(sp < LFS_DIR_TRAVERSE_DEPTH);
///                 // recurse, scan for duplicates, and update tag based on
///                 // creates/deletes
///                 stack[sp] = (struct lfs_dir_traverse){
///                     .dir        = dir,
///                     .off        = off,
///                     .ptag       = ptag,
///                     .attrs      = attrs,
///                     .attrcount  = attrcount,
///                     .tmask      = tmask,
///                     .ttag       = ttag,
///                     .begin      = begin,
///                     .end        = end,
///                     .diff       = diff,
///                     .cb         = cb,
///                     .data       = data,
///                     .tag        = tag,
///                     .buffer     = buffer,
///                     .disk       = disk,
///                 };
///                 sp += 1;
///
///                 tmask = 0;
///                 ttag = 0;
///                 begin = 0;
///                 end = 0;
///                 diff = 0;
///                 cb = lfs_dir_traverse_filter;
///                 data = &stack[sp-1].tag;
///                 continue;
///             }
///         }
///
/// popped:
///         // in filter range?
///         if (lfs_tag_id(tmask) != 0 &&
///                 !(lfs_tag_id(tag) >= begin && lfs_tag_id(tag) < end)) {
///             continue;
///         }
///
///         // handle special cases for mcu-side operations
///         if (lfs_tag_type3(tag) == LFS_FROM_NOOP) {
///             // do nothing
///         } else if (lfs_tag_type3(tag) == LFS_FROM_MOVE) {
///             // Without this condition, lfs_dir_traverse can exhibit an
///             // extremely expensive O(n^3) of nested loops when renaming.
///             // This happens because lfs_dir_traverse tries to filter tags by
///             // the tags in the source directory, triggering a second
///             // lfs_dir_traverse with its own filter operation.
///             //
///             // traverse with commit
///             // '-> traverse with filter
///             //     '-> traverse with move
///             //         '-> traverse with filter
///             //
///             // However we don't actually care about filtering the second set of
///             // tags, since duplicate tags have no effect when filtering.
///             //
///             // This check skips this unnecessary recursive filtering explicitly,
///             // reducing this runtime from O(n^3) to O(n^2).
///             if (cb == lfs_dir_traverse_filter) {
///                 continue;
///             }
///
///             // recurse into move
///             stack[sp] = (struct lfs_dir_traverse){
///                 .dir        = dir,
///                 .off        = off,
///                 .ptag       = ptag,
///                 .attrs      = attrs,
///                 .attrcount  = attrcount,
///                 .tmask      = tmask,
///                 .ttag       = ttag,
///                 .begin      = begin,
///                 .end        = end,
///                 .diff       = diff,
///                 .cb         = cb,
///                 .data       = data,
///                 .tag        = LFS_MKTAG(LFS_FROM_NOOP, 0, 0),
///             };
///             sp += 1;
///
///             uint16_t fromid = lfs_tag_size(tag);
///             uint16_t toid = lfs_tag_id(tag);
///             dir = buffer;
///             off = 0;
///             ptag = 0xffffffff;
///             attrs = NULL;
///             attrcount = 0;
///             tmask = LFS_MKTAG(0x600, 0x3ff, 0);
///             ttag = LFS_MKTAG(LFS_TYPE_STRUCT, 0, 0);
///             begin = fromid;
///             end = fromid+1;
///             diff = toid-fromid+diff;
///         } else if (lfs_tag_type3(tag) == LFS_FROM_USERATTRS) {
///             for (unsigned i = 0; i < lfs_tag_size(tag); i++) {
///                 const struct lfs_attr *a = buffer;
///                 res = cb(data, LFS_MKTAG(LFS_TYPE_USERATTR + a[i].type,
///                         lfs_tag_id(tag) + diff, a[i].size), a[i].buffer);
///                 if (res < 0) {
///                     return res;
///                 }
///
///                 if (res) {
///                     break;
///                 }
///             }
///         } else {
///             res = cb(data, tag + LFS_MKTAG(0, diff, 0), buffer);
///             if (res < 0) {
///                 return res;
///             }
///
///             if (res) {
///                 break;
///             }
///         }
///     }
///
///     if (sp > 0) {
///         // pop from the stack and return, fortunately all pops share
///         // a destination
///         dir         = stack[sp-1].dir;
///         off         = stack[sp-1].off;
///         ptag        = stack[sp-1].ptag;
///         attrs       = stack[sp-1].attrs;
///         attrcount   = stack[sp-1].attrcount;
///         tmask       = stack[sp-1].tmask;
///         ttag        = stack[sp-1].ttag;
///         begin       = stack[sp-1].begin;
///         end         = stack[sp-1].end;
///         diff        = stack[sp-1].diff;
///         cb          = stack[sp-1].cb;
///         data        = stack[sp-1].data;
///         tag         = stack[sp-1].tag;
///         buffer      = stack[sp-1].buffer;
///         disk        = stack[sp-1].disk;
///         sp -= 1;
///         goto popped;
///     } else {
///         return res;
///     }
/// }
/// #endif
///
/// ```
/// Helper: single place where the traverse callback is invoked.
/// C: `res = cb(data, tag + LFS_MKTAG(0, diff, 0), buffer);`
#[inline(always)]
fn dispatch_tag(
    cb: unsafe extern "C" fn(*mut core::ffi::c_void, lfs_tag_t, *const core::ffi::c_void) -> i32,
    data: *mut core::ffi::c_void,
    tag: lfs_tag_t,
    buffer: *const core::ffi::c_void,
    diff: i16,
) -> i32 {
    use crate::tag::lfs_mktag;
    let out_tag = tag.wrapping_add(lfs_mktag(0, diff as u32, 0));
    unsafe { cb(data, out_tag, buffer) }
}

pub fn lfs_dir_traverse(
    lfs: *mut crate::fs::Lfs,
    dir: *const LfsMdir,
    off: lfs_off_t,
    ptag: lfs_tag_t,
    attrs: *const core::ffi::c_void,
    attrcount: i32,
    tmask: lfs_tag_t,
    ttag: lfs_tag_t,
    begin: u16,
    end: u16,
    diff: i16,
    cb: Option<
        unsafe extern "C" fn(*mut core::ffi::c_void, lfs_tag_t, *const core::ffi::c_void) -> i32,
    >,
    data: *mut core::ffi::c_void,
) -> i32 {
    use crate::bd::bd::lfs_bd_read;
    use crate::lfs_type::lfs_type::LFS_FROM_NOOP;
    use crate::tag::{lfs_mktag, lfs_tag_dsize, lfs_tag_id, lfs_tag_type3};
    use crate::types::lfs_tag_t;
    use crate::util::lfs_frombe32;

    let cb = match cb {
        Some(c) => c,
        None => return 0,
    };

    let mut stack: [core::mem::MaybeUninit<LfsDirTraverseStack>; LFS_DIR_TRAVERSE_DEPTH - 1] =
        core::array::from_fn(|_| core::mem::MaybeUninit::uninit());
    let mut sp: usize = 0;
    let mut res: i32 = 0;

    let mut dir = dir;
    let mut off = off;
    let mut ptag = ptag;
    let mut attr_i: usize = 0;
    let mut tmask = tmask;
    let mut ttag = ttag;
    let mut begin = begin;
    let mut end = end;
    let mut diff = diff;
    let mut cb = cb;
    let mut data = data;
    let mut use_empty_attrs = false;

    let attrs_slice = if attrcount > 0 && !attrs.is_null() {
        unsafe {
            core::slice::from_raw_parts(attrs as *const crate::tag::lfs_mattr, attrcount as usize)
        }
    } else {
        &[]
    };

    let mut disk = crate::tag::lfs_diskoff { block: 0, off: 0 };

    let mask = lfs_mktag(0x7ff, 0, 0);

    let mut phase = TraversePhase::GetNextTag;

    #[cfg(feature = "loop_limits")]
    const MAX_TRAVERSE_PHASE_ITER: u32 = 65536;
    #[cfg(feature = "loop_limits")]
    let mut phase_iter: u32 = 0;
    loop {
        #[cfg(feature = "loop_limits")]
        {
            if phase_iter >= MAX_TRAVERSE_PHASE_ITER {
                panic!(
                    "loop_limits: MAX_TRAVERSE_PHASE_ITER ({}) exceeded in lfs_dir_traverse",
                    MAX_TRAVERSE_PHASE_ITER
                );
            }
            phase_iter += 1;
        }
        match phase {
            TraversePhase::GetNextTag => {
                crate::lfs_trace!("traverse GetNextTag: sp={} phase=GetNextTag", sp);
                // Per C: get next tag from disk or attrs. Never pop here.
                // Pop only happens in PopAndProcess (after exhaust or callback res!=0).
                let (tag, buffer) = {
                    let dir_ref = unsafe { &*dir };

                    if off + lfs_tag_dsize(ptag) < dir_ref.off {
                        crate::lfs_trace!(
                            "traverse GetNextTag: reading from disk dir.pair[0]={} off={}",
                            dir_ref.pair[0],
                            off
                        );
                        // Per C: advance off first to skip previous tag's data, then read
                        off += lfs_tag_dsize(ptag);
                        let mut tag_raw: lfs_tag_t = 0;
                        let err = lfs_bd_read(
                            lfs,
                            core::ptr::null_mut(),
                            unsafe { &mut (*lfs).rcache },
                            core::mem::size_of::<lfs_tag_t>() as u32,
                            dir_ref.pair[0],
                            off,
                            &mut tag_raw as *mut _ as *mut u8,
                            core::mem::size_of::<lfs_tag_t>() as u32,
                        );
                        if err != 0 {
                            return crate::lfs_pass_err!(err);
                        }
                        let tag_val = (lfs_frombe32(tag_raw) ^ ptag) | 0x8000_0000;
                        disk = crate::tag::lfs_diskoff {
                            block: dir_ref.pair[0],
                            off: off + 4,
                        };
                        ptag = tag_val;
                        (tag_val, &disk as *const _ as *const core::ffi::c_void)
                    } else if attr_i
                        < (if use_empty_attrs {
                            EMPTY_ATTRS
                        } else {
                            attrs_slice
                        })
                        .len()
                    {
                        let current_attrs = if use_empty_attrs {
                            EMPTY_ATTRS
                        } else {
                            attrs_slice
                        };
                        let attr = &current_attrs[attr_i];
                        crate::lfs_trace!(
                            "traverse GetNextTag: from attrs attr_i={} tag=0x{:08x} attrs_len={}",
                            attr_i,
                            attr.tag,
                            current_attrs.len()
                        );
                        attr_i += 1;
                        (attr.tag, attr.buffer)
                    } else {
                        res = 0;
                        if sp == 0 {
                            return res;
                        }
                        phase = TraversePhase::PopAndProcess;
                        continue;
                    }
                };

                if (mask & tmask & tag) != (mask & tmask & ttag) {
                    phase = TraversePhase::GetNextTag;
                } else if crate::tag::lfs_tag_id(tmask) != 0 {
                    crate::lfs_trace!(
                        "traverse GetNextTag: push tag=0x{:08x} type3={} buffer={:p} attr_i={}",
                        tag,
                        crate::tag::lfs_tag_type3(tag),
                        buffer,
                        attr_i
                    );
                    crate::lfs_assert!(sp < LFS_DIR_TRAVERSE_DEPTH);
                    unsafe {
                        let frame = LfsDirTraverseStack {
                            dir,
                            off,
                            ptag,
                            attr_i,
                            use_empty_attrs,
                            tmask,
                            ttag,
                            begin,
                            end,
                            diff,
                            cb,
                            data,
                            tag,
                            buffer,
                            disk,
                            redundant_tag: 0xffff_ffff,
                            redundant_buffer: core::ptr::null(),
                        };
                        stack[sp].write(frame);
                    }
                    sp += 1;
                    tmask = 0;
                    ttag = 0;
                    begin = 0;
                    end = 0;
                    diff = 0;
                    cb = lfs_dir_traverse_filter;
                    data = unsafe {
                        &mut (*stack[sp - 1].as_mut_ptr()).tag as *mut _ as *mut core::ffi::c_void
                    };
                    phase = TraversePhase::GetNextTag;
                } else {
                    phase = TraversePhase::ProcessTag {
                        tag,
                        buffer,
                        disk_override: None,
                    };
                }
            }
            TraversePhase::ProcessTag {
                tag,
                buffer,
                disk_override,
            } => {
                crate::lfs_trace!(
                    "traverse ProcessTag: sp={} tag=0x{:08x} type3={} buffer={:p}",
                    sp,
                    tag,
                    crate::tag::lfs_tag_type3(tag),
                    buffer
                );
                if crate::tag::lfs_tag_id(tmask) != 0
                    && !(crate::tag::lfs_tag_id(tag) >= begin && crate::tag::lfs_tag_id(tag) < end)
                {
                    phase = TraversePhase::GetNextTag;
                } else {
                    let type3 = lfs_tag_type3(tag);
                    if type3 == LFS_FROM_NOOP as u16 {
                        phase = TraversePhase::GetNextTag;
                    } else if type3 == crate::lfs_type::lfs_type::LFS_FROM_MOVE as u16 {
                        if core::ptr::eq(cb as *const (), lfs_dir_traverse_filter as *const ()) {
                            phase = TraversePhase::GetNextTag;
                        } else {
                            // Recurse into move: traverse source dir, process only tag with fromid.
                            // C: lfs.c lfs_dir_traverse LFS_FROM_MOVE branch.
                            let fromid = crate::tag::lfs_tag_size(tag) as u16;
                            let toid = crate::tag::lfs_tag_id(tag);
                            let new_diff = (toid as i16) - (fromid as i16) + diff;
                            crate::lfs_assert!(sp < LFS_DIR_TRAVERSE_DEPTH);
                            unsafe {
                                let noop_tag =
                                    lfs_mktag(crate::lfs_type::lfs_type::LFS_FROM_NOOP, 0, 0);
                                let frame = LfsDirTraverseStack {
                                    dir,
                                    off,
                                    ptag,
                                    attr_i,
                                    use_empty_attrs,
                                    tmask,
                                    ttag,
                                    begin,
                                    end,
                                    diff,
                                    cb,
                                    data,
                                    tag: noop_tag,
                                    buffer: core::ptr::null(),
                                    disk,
                                    redundant_tag: 0xffff_ffff,
                                    redundant_buffer: core::ptr::null(),
                                };
                                stack[sp].write(frame);
                            }
                            sp += 1;
                            dir = buffer as *const LfsMdir;
                            off = 0;
                            ptag = 0xffff_ffff;
                            attr_i = 0;
                            use_empty_attrs = true;
                            tmask = lfs_mktag(0x600, 0x3ff, 0);
                            ttag = lfs_mktag(crate::lfs_type::lfs_type::LFS_TYPE_STRUCT, 0, 0);
                            begin = fromid;
                            end = fromid + 1;
                            diff = new_diff;
                            phase = TraversePhase::GetNextTag;
                        }
                    } else if type3 == crate::lfs_type::lfs_type::LFS_FROM_USERATTRS as u16 {
                        // C: lfs.c:620-632 — iterate over user attrs, dispatch each to cb
                        let attr_count = crate::tag::lfs_tag_size(tag) as usize;
                        let attrs_ptr = buffer as *const crate::lfs_info::LfsAttr;
                        let mut i = 0;
                        while i < attr_count {
                            let a = unsafe { &*attrs_ptr.add(i) };
                            let userattr_tag = lfs_mktag(
                                crate::lfs_type::lfs_type::LFS_TYPE_USERATTR
                                    .wrapping_add(u32::from(a.type_)),
                                crate::tag::lfs_tag_id(tag) as u32 + diff as u32,
                                a.size,
                            );
                            res = dispatch_tag(
                                cb,
                                data,
                                userattr_tag,
                                a.buffer as *const core::ffi::c_void,
                                diff,
                            );
                            if res < 0 {
                                return res;
                            }
                            if res != 0 {
                                break;
                            }
                            i += 1;
                        }
                        if res == 0 {
                            phase = TraversePhase::GetNextTag;
                        } else if sp > 0 {
                            crate::lfs_trace!(
                                "traverse LFS_FROM_USERATTRS: res=1 storing redundant tag=0x{:08x}",
                                tag
                            );
                            unsafe {
                                (*stack[sp - 1].as_mut_ptr()).redundant_tag = tag;
                                (*stack[sp - 1].as_mut_ptr()).redundant_buffer = buffer;
                            }
                            phase = TraversePhase::PopAndProcess;
                        } else {
                            return res;
                        }
                    } else {
                        let actual_buffer = match disk_override {
                            Some(ref d) => d as *const _ as *const core::ffi::c_void,
                            None => buffer,
                        };
                        res = dispatch_tag(cb, data, tag, actual_buffer, diff);
                        if res < 0 {
                            return res;
                        }
                        if res != 0 {
                            if sp > 0 {
                                crate::lfs_trace!(
                                    "traverse ProcessTag: res=1 storing redundant tag=0x{:08x} buffer={:p}",
                                    tag,
                                    buffer
                                );
                                unsafe {
                                    (*stack[sp - 1].as_mut_ptr()).redundant_tag = tag;
                                    (*stack[sp - 1].as_mut_ptr()).redundant_buffer = buffer;
                                }
                                phase = TraversePhase::PopAndProcess;
                            } else {
                                return res;
                            }
                        } else {
                            phase = TraversePhase::GetNextTag;
                        }
                    }
                }
            }
            TraversePhase::PopAndProcess => {
                crate::lfs_trace!("traverse PopAndProcess: sp={}", sp);
                if sp == 0 {
                    return res;
                }
                let frame = unsafe { &*stack[sp - 1].as_ptr() };
                dir = frame.dir;
                off = frame.off;
                ptag = frame.ptag;
                attr_i = frame.attr_i;
                use_empty_attrs = frame.use_empty_attrs;
                tmask = frame.tmask;
                ttag = frame.ttag;
                begin = frame.begin;
                end = frame.end;
                diff = frame.diff;
                cb = frame.cb;
                data = frame.data;
                disk = frame.disk;
                // Per C: when filter marks redundant it sets *filtertag = NOOP. We pop and
                // dispatch that NOOP (emit nothing). The attr that triggered redundancy is
                // "given back" because we restore attr_i; we'll get it on the next GetNextTag
                // and emit it once. Using redundant_tag here would emit the attr now, but
                // we also restore attr_i, so we'd emit it again on next iteration = duplicate.
                //
                // When the tag was read from disk (0x80000000), frame.buffer pointed to the
                // outer `disk` variable, which gets overwritten by subsequent reads. Use the
                // copy saved in frame.disk so we pass the correct data to the callback.
                let proc_tag = frame.tag;
                let disk_override = if !crate::tag::lfs_tag_isvalid(frame.tag) {
                    Some(frame.disk)
                } else {
                    None
                };
                sp -= 1;
                phase = TraversePhase::ProcessTag {
                    tag: proc_tag,
                    buffer: frame.buffer,
                    disk_override,
                };
            }
        }
    }
}

// --- Test helpers for attr iteration validation ---

/// Output collected by lfs_dir_traverse_test_cb. Used to verify traverse passes correct buffers.
#[derive(Default)]
pub struct TraverseTestOut {
    pub call_count: u32,
    /// type3 per tag (full type: LFS_TYPE_*)
    pub tags: [u16; 8],
    pub first_bytes: [u8; 8],
}

pub unsafe extern "C" fn lfs_dir_traverse_test_cb(
    p: *mut core::ffi::c_void,
    tag: lfs_tag_t,
    buffer: *const core::ffi::c_void,
) -> i32 {
    use crate::tag::lfs_tag_type3;

    let out = p as *mut TraverseTestOut;
    if out.is_null() || (*out).call_count as usize >= 8 {
        return 0;
    }
    let i = (*out).call_count as usize;
    (*out).tags[i] = lfs_tag_type3(tag);
    (*out).first_bytes[i] = if buffer.is_null() {
        0
    } else {
        *((buffer as *const u8).add(0))
    };
    (*out).call_count += 1;
    0
}
