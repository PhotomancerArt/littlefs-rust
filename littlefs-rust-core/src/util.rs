//! Utility functions. Per lfs_util.h static inline and lfs.c small type-level utils.

use crate::types::{lfs_block_t, lfs_size_t};

/// Per lfs_util.h lfs_max (lines 129-131)
///
/// C:
/// ```c
/// static inline uint32_t lfs_max(uint32_t a, uint32_t b) {
///     return (a > b) ? a : b;
/// }
/// ```
#[inline(always)]
pub fn lfs_max(a: u32, b: u32) -> u32 {
    if a > b {
        a
    } else {
        b
    }
}

/// Per lfs_util.h lfs_min (lines 133-135)
///
/// C:
/// ```c
/// static inline uint32_t lfs_min(uint32_t a, uint32_t b) {
///     return (a < b) ? a : b;
/// }
/// ```
#[inline(always)]
pub fn lfs_min(a: u32, b: u32) -> u32 {
    if a < b {
        a
    } else {
        b
    }
}

/// Per lfs_util.h lfs_aligndown (lines 138-140)
///
/// C:
/// ```c
/// static inline uint32_t lfs_aligndown(uint32_t a, uint32_t alignment) {
///     return a - (a % alignment);
/// }
/// ```
#[inline(always)]
pub fn lfs_aligndown(a: u32, alignment: u32) -> u32 {
    a - (a % alignment)
}

/// Per lfs_util.h lfs_alignup (lines 142-144)
///
/// C:
/// ```c
/// static inline uint32_t lfs_alignup(uint32_t a, uint32_t alignment) {
///     return lfs_aligndown(a + alignment-1, alignment);
/// }
/// ```
#[inline(always)]
pub fn lfs_alignup(a: u32, alignment: u32) -> u32 {
    // uint32_t arithmetic in C wraps; lfs_dir_alloc relies on this when the
    // revision count read from an erased block is 0xffffffff.
    lfs_aligndown(a.wrapping_add(alignment).wrapping_sub(1), alignment)
}

/// Per lfs_util.h lfs_npw2 (lines 147-161) - smallest power of 2 >= a
///
/// C (fallback when LFS_NO_INTRINSICS):
/// ```c
/// static inline uint32_t lfs_npw2(uint32_t a) {
///     uint32_t r = 0;
///     uint32_t s;
///     a -= 1;
///     s = (a > 0xffff) << 4; a >>= s; r |= s;
///     s = (a > 0xff  ) << 3; a >>= s; r |= s;
///     s = (a > 0xf   ) << 2; a >>= s; r |= s;
///     s = (a > 0x3   ) << 1; a >>= s; r |= s;
///     return (r | (a >> 1)) + 1;
/// }
/// ```
#[inline(always)]
pub fn lfs_npw2(a: u32) -> u32 {
    let a = a.wrapping_sub(1);
    let s4 = if a > 0xffff { 1 } else { 0 };
    let a = a >> (s4 << 4);
    let s3 = if a > 0xff { 1 } else { 0 };
    let a = a >> (s3 << 3);
    let s2 = if a > 0xf { 1 } else { 0 };
    let a = a >> (s2 << 2);
    let s1 = if a > 0x3 { 1 } else { 0 };
    let a = a >> (s1 << 1);
    (s4 << 4 | s3 << 3 | s2 << 2 | s1 << 1 | (a >> 1)) + 1
}

/// Per lfs_util.h lfs_ctz (lines 164-170) - trailing zeros, lfs_ctz(0) may be undefined
///
/// C (fallback when LFS_NO_INTRINSICS):
/// ```c
/// static inline uint32_t lfs_ctz(uint32_t a) {
///     return lfs_npw2((a & -a) + 1) - 1;
/// }
/// ```
#[inline(always)]
pub fn lfs_ctz(a: u32) -> u32 {
    lfs_npw2((a & a.wrapping_neg()).wrapping_add(1)) - 1
}

/// Per lfs_util.h lfs_popc (lines 173-182) - population count
///
/// C (fallback when LFS_NO_INTRINSICS):
/// ```c
/// static inline uint32_t lfs_popc(uint32_t a) {
///     a = a - ((a >> 1) & 0x55555555);
///     a = (a & 0x33333333) + ((a >> 2) & 0x33333333);
///     return (((a + (a >> 4)) & 0xf0f0f0f) * 0x1010101) >> 24;
/// }
/// ```
#[inline(always)]
pub fn lfs_popc(a: u32) -> u32 {
    let a = a - ((a >> 1) & 0x5555_5555);
    let a = (a & 0x3333_3333) + ((a >> 2) & 0x3333_3333);
    (((a.wrapping_add(a >> 4)) & 0x0f0f_0f0f).wrapping_mul(0x0101_0101)) >> 24
}

/// Per lfs_util.h lfs_scmp (lines 186-188) - sequence comparison
///
/// C:
/// ```c
/// static inline int lfs_scmp(uint32_t a, uint32_t b) {
///     return (int)(unsigned)(a - b);
/// }
/// ```
#[inline(always)]
pub fn lfs_scmp(a: u32, b: u32) -> i32 {
    (a.wrapping_sub(b)) as i32
}

/// Per lfs_util.h lfs_fromle32 (lines 191-204) - little-endian to native
///
/// C:
/// ```c
/// static inline uint32_t lfs_fromle32(uint32_t a) {
///     // LE host: return a; BE: bswap or byte shuffle
///     return a;  // Rust uses u32::from_le
/// }
/// ```
#[inline(always)]
pub fn lfs_fromle32(a: u32) -> u32 {
    u32::from_le(a)
}

/// Per lfs_util.h lfs_tole32 (lines 206-208)
///
/// C:
/// ```c
/// static inline uint32_t lfs_tole32(uint32_t a) {
///     return lfs_fromle32(a);
/// }
/// ```
#[inline(always)]
pub fn lfs_tole32(a: u32) -> u32 {
    a.to_le()
}

/// Per lfs_util.h lfs_frombe32 (lines 211-226) - big-endian to native
///
/// C:
/// ```c
/// static inline uint32_t lfs_frombe32(uint32_t a) {
///     // platform-dependent; Rust uses u32::from_be
///     return a;
/// }
/// ```
#[inline(always)]
pub fn lfs_frombe32(a: u32) -> u32 {
    u32::from_be(a)
}

/// Per lfs_util.h lfs_tobe32 (lines 228-230)
///
/// C:
/// ```c
/// static inline uint32_t lfs_tobe32(uint32_t a) {
///     return lfs_frombe32(a);
/// }
/// ```
#[inline(always)]
pub fn lfs_tobe32(a: u32) -> u32 {
    a.to_be()
}

// --- lfs.c path operations ---

/// Per C strspn: count leading bytes equal to `c`, stop at first unequal or null.
#[inline(always)]
pub fn lfs_strspn(p: *const u8, c: u8) -> u32 {
    if p.is_null() {
        return 0;
    }
    let mut n: u32 = 0;
    unsafe {
        let mut q = p;
        #[cfg(feature = "loop_limits")]
        const MAX_STRSPN_ITER: u32 = 4096;
        #[cfg(feature = "loop_limits")]
        let mut iter: u32 = 0;
        while *q == c {
            #[cfg(feature = "loop_limits")]
            {
                if iter >= MAX_STRSPN_ITER {
                    panic!(
                        "loop_limits: MAX_STRSPN_ITER ({}) exceeded",
                        MAX_STRSPN_ITER
                    );
                }
                iter += 1;
            }
            n += 1;
            q = q.add(1);
        }
    }
    n
}

/// Per C strcspn: count bytes until we hit `c` or null.
#[inline(always)]
pub fn lfs_strcspn(p: *const u8, c: u8) -> u32 {
    if p.is_null() {
        return 0;
    }
    let mut n: u32 = 0;
    unsafe {
        let mut q = p;
        #[cfg(feature = "loop_limits")]
        const MAX_STRCSPN_ITER: u32 = 4096;
        #[cfg(feature = "loop_limits")]
        let mut iter: u32 = 0;
        while *q != c && *q != 0 {
            #[cfg(feature = "loop_limits")]
            {
                if iter >= MAX_STRCSPN_ITER {
                    panic!(
                        "loop_limits: MAX_STRCSPN_ITER ({}) exceeded",
                        MAX_STRCSPN_ITER
                    );
                }
                iter += 1;
            }
            n += 1;
            q = q.add(1);
        }
    }
    n
}

/// Per C: slice from NUL-terminated string. Max 256 bytes.
#[inline(always)]
pub fn lfs_path_slice_from_cstr(p: *const u8) -> &'static [u8] {
    if p.is_null() {
        return &[];
    }
    unsafe {
        let mut len = 0;
        while len < 256 && *p.add(len) != 0 {
            len += 1;
        }
        core::slice::from_raw_parts(p, len)
    }
}

/// Per lfs.c lfs_path_namelen (lines 289-291)
///
/// C:
/// ```c
/// static inline lfs_size_t lfs_path_namelen(const char *path) {
///     return strcspn(path, "/");
/// }
/// ```
#[inline(always)]
pub fn lfs_path_namelen(path: &[u8]) -> u32 {
    path.iter().position(|&b| b == b'/').unwrap_or(path.len()) as lfs_size_t
}

/// Per lfs.c lfs_path_islast (lines 293-296)
///
/// C:
/// ```c
/// static inline bool lfs_path_islast(const char *path) {
///     lfs_size_t namelen = lfs_path_namelen(path);
///     return path[namelen + strspn(path + namelen, "/")] == '\0';
/// }
/// ```
#[inline(always)]
pub fn lfs_path_islast(path: &[u8]) -> bool {
    let namelen = lfs_path_namelen(path) as usize;
    let rest = path.get(namelen..).unwrap_or(&[]);
    let skip = rest.iter().take_while(|&&b| b == b'/').count();
    path.get(namelen + skip).is_none_or(|&b| b == 0)
}

/// Per lfs.c lfs_path_isdir (lines 298-300)
///
/// C:
/// ```c
/// static inline bool lfs_path_isdir(const char *path) {
///     return path[lfs_path_namelen(path)] != '\0';
/// }
/// ```
#[inline(always)]
pub fn lfs_path_isdir(path: &[u8]) -> bool {
    let namelen = lfs_path_namelen(path) as usize;
    path.get(namelen).is_some_and(|&b| b != 0)
}

/// Per lfs.c lfs_pair_fromle32 (lines 326-329)
///
/// C:
/// ```c
/// static inline void lfs_pair_fromle32(lfs_block_t pair[2]) {
///     pair[0] = lfs_fromle32(pair[0]);
///     pair[1] = lfs_fromle32(pair[1]);
/// }
/// ```
#[inline(always)]
pub fn lfs_pair_fromle32(pair: &mut [lfs_block_t; 2]) {
    pair[0] = lfs_fromle32(pair[0]);
    pair[1] = lfs_fromle32(pair[1]);
}

/// Per lfs.c lfs_pair_tole32 (lines 333-336)
///
/// C:
/// ```c
/// static inline void lfs_pair_tole32(lfs_block_t pair[2]) {
///     pair[0] = lfs_tole32(pair[0]);
///     pair[1] = lfs_tole32(pair[1]);
/// }
/// ```
#[inline(always)]
pub fn lfs_pair_tole32(pair: &mut [lfs_block_t; 2]) {
    pair[0] = lfs_tole32(pair[0]);
    pair[1] = lfs_tole32(pair[1]);
}

/// Per lfs.c lfs_pair_swap (lines 302-306)
///
/// C:
/// ```c
/// static inline void lfs_pair_swap(lfs_block_t pair[2]) {
///     lfs_block_t t = pair[0];
///     pair[0] = pair[1];
///     pair[1] = t;
/// }
/// ```
#[inline(always)]
pub fn lfs_pair_swap(pair: &mut [lfs_block_t; 2]) {
    pair.swap(0, 1);
}

/// Per lfs.c lfs_pair_isnull (lines 308-310)
///
/// C:
/// ```c
/// static inline bool lfs_pair_isnull(const lfs_block_t pair[2]) {
///     return pair[0] == LFS_BLOCK_NULL || pair[1] == LFS_BLOCK_NULL;
/// }
/// ```
#[inline(always)]
pub fn lfs_pair_isnull(pair: &[lfs_block_t; 2]) -> bool {
    use crate::types::LFS_BLOCK_NULL;
    pair[0] == LFS_BLOCK_NULL || pair[1] == LFS_BLOCK_NULL
}

/// Per lfs.c lfs_pair_cmp (lines 312-317) - returns 0 if equal
///
/// C:
/// ```c
/// static inline int lfs_pair_cmp(const lfs_block_t paira[2], const lfs_block_t pairb[2]) {
///     return !(paira[0] == pairb[0] || paira[1] == pairb[1] ||
///              paira[0] == pairb[1] || paira[1] == pairb[0]);
/// }
/// ```
#[inline(always)]
pub fn lfs_pair_cmp(paira: &[lfs_block_t; 2], pairb: &[lfs_block_t; 2]) -> i32 {
    let eq = paira[0] == pairb[0]
        || paira[1] == pairb[1]
        || paira[0] == pairb[1]
        || paira[1] == pairb[0];
    if eq {
        0
    } else {
        1
    }
}

/// Per lfs.c lfs_pair_issync (lines 319-324)
///
/// C:
/// ```c
/// static inline bool lfs_pair_issync(const lfs_block_t paira[2], const lfs_block_t pairb[2]) {
///     return (paira[0] == pairb[0] && paira[1] == pairb[1]) ||
///            (paira[0] == pairb[1] && paira[1] == pairb[0]);
/// }
/// ```
#[inline(always)]
pub fn lfs_pair_issync(paira: &[lfs_block_t; 2], pairb: &[lfs_block_t; 2]) -> bool {
    (paira[0] == pairb[0] && paira[1] == pairb[1]) || (paira[0] == pairb[1] && paira[1] == pairb[0])
}
