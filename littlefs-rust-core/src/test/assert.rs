//! Assertion helpers for superblock magic and block content.

use crate::test::ram::{MAGIC, MAGIC_OFFSET};
use crate::LfsConfig;

/// Read config's block at offset 0, return magic region (8 bytes at MAGIC_OFFSET).
fn read_magic_region(config: *const LfsConfig, block: u32) -> Option<[u8; 8]> {
    let mut buf = [0u8; 24];
    let err = unsafe {
        let read = (*config).read.expect("read callback");
        read(config, block, 0, buf.as_mut_ptr(), buf.len() as u32)
    };
    if err != 0 {
        return None;
    }
    Some(buf[MAGIC_OFFSET as usize..][..8].try_into().unwrap())
}

/// Panics if block does not contain MAGIC at MAGIC_OFFSET.
pub fn assert_block_has_magic(config: *const LfsConfig, block: u32) {
    let got = read_magic_region(config, block)
        .unwrap_or_else(|| panic!("read_block_raw failed for block {}", block));
    assert_eq!(
        &got, MAGIC,
        "block {}: expected MAGIC at offset {}, got {:?}",
        block, MAGIC_OFFSET, got
    );
}

/// Panics if blocks 0 or 1 do not contain MAGIC.
pub fn assert_blocks_0_and_1_have_magic(config: *const LfsConfig) {
    assert_block_has_magic(config, 0);
    assert_block_has_magic(config, 1);
}
