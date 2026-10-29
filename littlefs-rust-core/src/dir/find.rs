//! Directory find. Per lfs.c lfs_dir_find, lfs_dir_find_match.

use crate::bd::bd::lfs_bd_cmp;
use crate::dir::fetch::lfs_dir_fetchmatch;
use crate::dir::traverse::lfs_dir_get;
use crate::dir::LfsMdir;
use crate::error::{LFS_ERR_INVAL, LFS_ERR_NOENT, LFS_ERR_NOTDIR};
use crate::fs::Lfs;
use crate::lfs_type::lfs_type::{LFS_TYPE_DIR, LFS_TYPE_NAME, LFS_TYPE_STRUCT};
use crate::tag::{lfs_diskoff, lfs_mktag, lfs_tag_id, lfs_tag_size, lfs_tag_type3};
use crate::types::{lfs_size_t, lfs_tag_t};
use crate::util::{lfs_min, lfs_pair_fromle32, lfs_strcspn, lfs_strspn};

// Per lfs.c enum: LFS_CMP_EQ=0, LFS_CMP_LT=1, LFS_CMP_GT=2 (positive = not error)
const LFS_CMP_EQ: i32 = 0;
const LFS_CMP_LT: i32 = 1;
const LFS_CMP_GT: i32 = 2;

/// Per lfs.c struct lfs_dir_find_match (lines 1447-1475)
#[repr(C)]
pub struct LfsDirFindMatch {
    pub lfs: *mut Lfs,
    pub name: *const u8,
    pub size: lfs_size_t,
}

/// Per lfs.c lfs_dir_find_match (and struct lfs_dir_find_match) (lines 1447-1475)
///
/// C:
/// ```c
/// struct lfs_dir_find_match {
///     lfs_t *lfs;
///     const void *name;
///     lfs_size_t size;
/// };
///
/// static int lfs_dir_find_match(void *data,
///         lfs_tag_t tag, const void *buffer) {
///     struct lfs_dir_find_match *name = data;
///     lfs_t *lfs = name->lfs;
///     const struct lfs_diskoff *disk = buffer;
///
///     // compare with disk
///     lfs_size_t diff = lfs_min(name->size, lfs_tag_size(tag));
///     int res = lfs_bd_cmp(lfs,
///             NULL, &lfs->rcache, diff,
///             disk->block, disk->off, name->name, diff);
///     if (res != LFS_CMP_EQ) {
///         return res;
///     }
///
///     // only equal if our size is still the same
///     if (name->size != lfs_tag_size(tag)) {
///         return (name->size < lfs_tag_size(tag)) ? LFS_CMP_LT : LFS_CMP_GT;
///     }
///
///     // found a match!
///     return LFS_CMP_EQ;
/// }
///
/// ```
pub unsafe extern "C" fn lfs_dir_find_match(
    data: *mut core::ffi::c_void,
    tag: lfs_tag_t,
    buffer: *const core::ffi::c_void,
) -> i32 {
    if data.is_null() || buffer.is_null() {
        return LFS_CMP_LT;
    }
    unsafe {
        let name = &*(data as *const LfsDirFindMatch);
        let disk = &*(buffer as *const lfs_diskoff);
        let lfs = &mut *name.lfs;

        let diff = lfs_min(name.size, lfs_tag_size(tag));
        let res = lfs_bd_cmp(
            name.lfs,
            core::ptr::null(),
            &mut lfs.rcache,
            diff,
            disk.block,
            disk.off,
            name.name,
            diff,
        );
        if res != LFS_CMP_EQ {
            return res;
        }
        if name.size != lfs_tag_size(tag) {
            return if name.size < lfs_tag_size(tag) {
                LFS_CMP_LT
            } else {
                LFS_CMP_GT
            };
        }
        LFS_CMP_EQ
    }
}

/// Per lfs.c lfs_dir_find (lines 1483-1590)
///
/// C:
/// ```c
/// static lfs_stag_t lfs_dir_find(lfs_t *lfs, lfs_mdir_t *dir,
///         const char **path, uint16_t *id) {
///     // we reduce path to a single name if we can find it
///     const char *name = *path;
///
///     // default to root dir
///     lfs_stag_t tag = LFS_MKTAG(LFS_TYPE_DIR, 0x3ff, 0);
///     dir->tail[0] = lfs->root[0];
///     dir->tail[1] = lfs->root[1];
///
///     // empty paths are not allowed
///     if (*name == '\0') {
///         return LFS_ERR_INVAL;
///     }
///
///     while (true) {
/// nextname:
///         // skip slashes if we're a directory
///         if (lfs_tag_type3(tag) == LFS_TYPE_DIR) {
///             name += strspn(name, "/");
///         }
///         lfs_size_t namelen = strcspn(name, "/");
///
///         // skip '.'
///         if (namelen == 1 && memcmp(name, ".", 1) == 0) {
///             name += namelen;
///             goto nextname;
///         }
///
///         // error on unmatched '..', trying to go above root?
///         if (namelen == 2 && memcmp(name, "..", 2) == 0) {
///             return LFS_ERR_INVAL;
///         }
///
///         // skip if matched by '..' in name
///         const char *suffix = name + namelen;
///         lfs_size_t sufflen;
///         int depth = 1;
///         while (true) {
///             suffix += strspn(suffix, "/");
///             sufflen = strcspn(suffix, "/");
///             if (sufflen == 0) {
///                 break;
///             }
///
///             if (sufflen == 1 && memcmp(suffix, ".", 1) == 0) {
///                 // noop
///             } else if (sufflen == 2 && memcmp(suffix, "..", 2) == 0) {
///                 depth -= 1;
///                 if (depth == 0) {
///                     name = suffix + sufflen;
///                     goto nextname;
///                 }
///             } else {
///                 depth += 1;
///             }
///
///             suffix += sufflen;
///         }
///
///         // found path
///         if (*name == '\0') {
///             return tag;
///         }
///
///         // update what we've found so far
///         *path = name;
///
///         // only continue if we're a directory
///         if (lfs_tag_type3(tag) != LFS_TYPE_DIR) {
///             return LFS_ERR_NOTDIR;
///         }
///
///         // grab the entry data
///         if (lfs_tag_id(tag) != 0x3ff) {
///             lfs_stag_t res = lfs_dir_get(lfs, dir, LFS_MKTAG(0x700, 0x3ff, 0),
///                     LFS_MKTAG(LFS_TYPE_STRUCT, lfs_tag_id(tag), 8), dir->tail);
///             if (res < 0) {
///                 return res;
///             }
///             lfs_pair_fromle32(dir->tail);
///         }
///
///         // find entry matching name
///         while (true) {
///             tag = lfs_dir_fetchmatch(lfs, dir, dir->tail,
///                     LFS_MKTAG(0x780, 0, 0),
///                     LFS_MKTAG(LFS_TYPE_NAME, 0, namelen),
///                     id,
///                     lfs_dir_find_match, &(struct lfs_dir_find_match){
///                         lfs, name, namelen});
///             if (tag < 0) {
///                 return tag;
///             }
///
///             if (tag) {
///                 break;
///             }
///
///             if (!dir->split) {
///                 return LFS_ERR_NOENT;
///             }
///         }
///
///         // to next name
///         name += namelen;
///     }
/// }
/// ```
pub fn lfs_dir_find(
    lfs: *mut Lfs,
    dir: *mut LfsMdir,
    path: *mut *const u8,
    id: *mut u16,
) -> crate::types::lfs_stag_t {
    if lfs.is_null() || dir.is_null() || path.is_null() {
        return crate::lfs_err!(LFS_ERR_INVAL as crate::types::lfs_stag_t);
    }
    unsafe {
        let lfs_ref = lfs;
        let dir_ref = dir;
        let mut name = *path;
        if name.is_null() {
            return crate::lfs_err!(LFS_ERR_INVAL as crate::types::lfs_stag_t);
        }

        // C: lfs.c:1488-1491
        let mut tag = lfs_mktag(LFS_TYPE_DIR, 0x3ff, 0) as i32;
        (*dir_ref).tail[0] = (*lfs_ref).root[0];
        (*dir_ref).tail[1] = (*lfs_ref).root[1];

        // C: lfs.c:1494-1495
        if *name == 0 {
            return crate::lfs_err!(LFS_ERR_INVAL as crate::types::lfs_stag_t);
        }

        'nextname: loop {
            // C: nextname - lfs.c:1510-1512
            if u32::from(lfs_tag_type3(tag as u32)) == LFS_TYPE_DIR {
                let skip = lfs_strspn(name, b'/');
                name = name.add(skip as usize);
            }
            let namelen = lfs_strcspn(name, b'/');

            // C: lfs.c:1516-1519 - skip '.'
            if namelen == 1 && *name == b'.' {
                name = name.add(1);
                continue;
            }

            // C: lfs.c:1522-1524 - error on '..' at top level
            if namelen == 2 && *name == b'.' && *name.add(1) == b'.' {
                return crate::lfs_err!(LFS_ERR_INVAL as crate::types::lfs_stag_t);
            }

            // C: lfs.c:1527-1541 - skip if matched by '..' in path
            let mut suffix = name.add(namelen as usize);
            let mut depth: i32 = 1;
            #[cfg(feature = "loop_limits")]
            const MAX_PATH_DEPTH_ITER: u32 = 512;
            #[cfg(feature = "loop_limits")]
            let mut path_iter: u32 = 0;
            loop {
                #[cfg(feature = "loop_limits")]
                {
                    if path_iter >= MAX_PATH_DEPTH_ITER {
                        panic!(
                            "loop_limits: MAX_PATH_DEPTH_ITER ({}) exceeded in path .. parsing",
                            MAX_PATH_DEPTH_ITER
                        );
                    }
                    path_iter += 1;
                }
                let suffix_skip = lfs_strspn(suffix, b'/');
                suffix = suffix.add(suffix_skip as usize);
                let sufflen = lfs_strcspn(suffix, b'/');
                if sufflen == 0 {
                    break;
                }
                if sufflen == 1 && *suffix == b'.' {
                    // noop
                } else if sufflen == 2 && *suffix == b'.' && *suffix.add(1) == b'.' {
                    depth -= 1;
                    if depth == 0 {
                        name = suffix.add(sufflen as usize);
                        continue 'nextname;
                    }
                } else {
                    depth += 1;
                }
                suffix = suffix.add(sufflen as usize);
            }

            // C: lfs.c:1544-1546 - found path
            if *name == 0 {
                return tag;
            }

            // C: lfs.c:1549
            *path = name;

            // C: lfs.c:1652-1654
            if u32::from(lfs_tag_type3(tag as u32)) != LFS_TYPE_DIR {
                return crate::lfs_err!(LFS_ERR_NOTDIR as crate::types::lfs_stag_t);
            }

            // C: lfs.c:1557-1564
            if lfs_tag_id(tag as u32) != 0x3ff {
                let res = lfs_dir_get(
                    lfs,
                    dir as *const _,
                    lfs_mktag(0x700, 0x3ff, 0),
                    lfs_mktag(LFS_TYPE_STRUCT, lfs_tag_id(tag as u32) as u32, 8),
                    (*dir_ref).tail.as_mut_ptr() as *mut core::ffi::c_void,
                );
                if res < 0 {
                    return res;
                }
                lfs_pair_fromle32(&mut (*dir_ref).tail);
            }

            // C: lfs.c:1567-1584 - find entry matching name
            #[cfg(feature = "loop_limits")]
            const MAX_FIND_ITER: u32 = 256;
            #[cfg(feature = "loop_limits")]
            let mut find_iter: u32 = 0;
            loop {
                #[cfg(feature = "loop_limits")]
                {
                    find_iter += 1;
                    if find_iter > MAX_FIND_ITER {
                        panic!(
                            "loop_limits: MAX_FIND_ITER ({}) exceeded name_len={} tail={:?}",
                            MAX_FIND_ITER,
                            namelen,
                            (*dir_ref).tail
                        );
                    }
                }
                #[cfg(feature = "loop_limits")]
                crate::lfs_trace!(
                    "dir_find: iter={} tag={} split={} tail=[{},{}] namelen={}",
                    find_iter,
                    tag,
                    (*dir_ref).split,
                    (*dir_ref).tail[0],
                    (*dir_ref).tail[1],
                    namelen
                );
                let mut match_data = LfsDirFindMatch {
                    lfs,
                    name,
                    size: namelen,
                };
                tag = lfs_dir_fetchmatch(
                    lfs as *mut _ as *const core::ffi::c_void,
                    dir_ref,
                    &(*dir_ref).tail as *const _,
                    lfs_mktag(0x780, 0, 0),
                    lfs_mktag(LFS_TYPE_NAME, 0, namelen),
                    id,
                    Some(lfs_dir_find_match),
                    &mut match_data as *mut _ as *mut core::ffi::c_void,
                );
                if tag < 0 {
                    return tag;
                }
                if tag != 0 {
                    break;
                }
                if !(*dir_ref).split {
                    return crate::lfs_err!(LFS_ERR_NOENT as crate::types::lfs_stag_t);
                }
            }

            name = name.add(namelen as usize);
        }
    }
}
