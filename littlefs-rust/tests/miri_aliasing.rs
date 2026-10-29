//! Small safe-API regressions intended to run under Miri as well as normal tests.
//! Run with `cargo +nightly miri test -p littlefs-rust --test miri_aliasing`;
//! repeat with `MIRIFLAGS=-Zmiri-tree-borrows` for the second aliasing model.

use littlefs_rust::{Config, Filesystem, RamStorage};

#[test]
fn format_mount_write_and_read() {
    let mut storage = RamStorage::new(4096, 32);
    let config = Config::new(4096, 32);
    Filesystem::format(&mut storage, &config).expect("format storage");
    let fs = Filesystem::mount(storage, config)
        .unwrap_or_else(|(error, _)| panic!("mount formatted storage: {error}"));
    fs.write_file("/episode", &[42; 8192])
        .expect("write episode");
    assert_eq!(
        fs.read_to_vec("/episode").expect("read episode"),
        [42; 8192]
    );
    fs.unmount().expect("unmount storage");
}

#[test]
fn open_handles_survive_directory_changes() {
    use littlefs_rust::OpenFlags;
    let mut storage = RamStorage::new(4096, 32);
    let config = Config::new(4096, 32);
    Filesystem::format(&mut storage, &config).expect("format storage");
    let fs = Filesystem::mount(storage, config)
        .unwrap_or_else(|(error, _)| panic!("mount formatted storage: {error}"));
    fs.mkdir("/episodes").expect("create episodes directory");
    let file = fs
        .open("/episodes/one", OpenFlags::WRITE | OpenFlags::CREATE)
        .expect("open first episode");
    file.write(b"one").expect("write first episode");
    let mut directory = fs.read_dir("/episodes").expect("open directory");
    assert_eq!(
        directory
            .next()
            .expect("first entry")
            .expect("read entry")
            .name,
        "one"
    );
    fs.write_file("/episodes/two", b"two")
        .expect("create second episode with handles open");
    file.sync()
        .expect("sync first episode after directory change");
    file.close().expect("close first episode");
    directory.close().expect("close directory");
    assert_eq!(
        fs.read_to_vec("/episodes/one").expect("read first episode"),
        b"one"
    );
    assert_eq!(
        fs.read_to_vec("/episodes/two")
            .expect("read second episode"),
        b"two"
    );
    fs.unmount().expect("unmount storage");
}
