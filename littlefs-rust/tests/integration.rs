use std::string::String;

use littlefs_rust::{
    Config, Error, FileType, Filesystem, OpenFlags, RamStorage, SeekFrom, Storage,
};

fn format_and_mount() -> Filesystem<RamStorage> {
    let mut storage = RamStorage::new(512, 128);
    let config = Config::new(512, 128);
    Filesystem::format(&mut storage, &config).expect("format");
    Filesystem::mount(storage, config)
        .map_err(|(e, _)| e)
        .expect("mount")
}

#[test]
fn test_format_mount_unmount() {
    let mut storage = RamStorage::new(512, 128);
    let config = Config::new(512, 128);
    Filesystem::format(&mut storage, &config).unwrap();
    let fs = Filesystem::mount(storage, config)
        .map_err(|(e, _)| e)
        .unwrap();
    let _storage = fs.unmount().unwrap();
}

#[test]
fn test_mount_unformatted_fails() {
    let storage = RamStorage::new(512, 128);
    let config = Config::new(512, 128);
    let result = Filesystem::mount(storage, config);
    let (err, recovered) = result.err().expect("mount should fail");
    assert_eq!(err, Error::Corrupt);
    assert_eq!(recovered.block_size(), 512);
}

#[test]
fn test_drop_unmounts() {
    let mut storage = RamStorage::new(512, 128);
    let config = Config::new(512, 128);
    Filesystem::format(&mut storage, &config).unwrap();
    {
        let _fs = Filesystem::mount(storage, config)
            .map_err(|(e, _)| e)
            .unwrap();
    }
    // No panic — Drop ran unmount
}

#[test]
fn test_format_does_not_consume_storage() {
    let mut storage = RamStorage::new(512, 128);
    let config = Config::new(512, 128);
    Filesystem::format(&mut storage, &config).unwrap();
    assert_eq!(storage.block_size(), 512);
}

#[test]
fn test_write_read_roundtrip() {
    let fs = format_and_mount();
    let data = b"Hello, littlefs!";

    let file = fs
        .open("/hello.txt", OpenFlags::WRITE | OpenFlags::CREATE)
        .unwrap();
    file.write(data).unwrap();
    file.close().unwrap();

    let file = fs.open("/hello.txt", OpenFlags::READ).unwrap();
    let mut buf = vec![0u8; 64];
    let n = file.read(&mut buf).unwrap();
    assert_eq!(&buf[..n as usize], data);
    file.close().unwrap();

    fs.unmount().unwrap();
}

#[test]
fn test_multiple_open_files() {
    let fs = format_and_mount();

    let f1 = fs
        .open("/a.txt", OpenFlags::WRITE | OpenFlags::CREATE)
        .unwrap();
    let f2 = fs
        .open("/b.txt", OpenFlags::WRITE | OpenFlags::CREATE)
        .unwrap();

    f1.write(b"aaa").unwrap();
    f2.write(b"bbb").unwrap();

    f1.close().unwrap();
    f2.close().unwrap();

    let data_a = fs.read_to_vec("/a.txt").unwrap();
    let data_b = fs.read_to_vec("/b.txt").unwrap();
    assert_eq!(data_a, b"aaa");
    assert_eq!(data_b, b"bbb");

    fs.unmount().unwrap();
}

#[test]
fn test_seek_tell_size() {
    let fs = format_and_mount();

    let file = fs
        .open("/data.bin", OpenFlags::WRITE | OpenFlags::CREATE)
        .unwrap();
    file.write(b"0123456789").unwrap();
    file.close().unwrap();

    let file = fs.open("/data.bin", OpenFlags::READ).unwrap();
    assert_eq!(file.size(), 10);
    assert_eq!(file.tell(), 0);

    file.seek(SeekFrom::Start(5)).unwrap();
    assert_eq!(file.tell(), 5);

    let mut buf = [0u8; 5];
    let n = file.read(&mut buf).unwrap();
    assert_eq!(n, 5);
    assert_eq!(&buf, b"56789");

    file.seek(SeekFrom::End(-3)).unwrap();
    assert_eq!(file.tell(), 7);

    file.close().unwrap();
    fs.unmount().unwrap();
}

#[test]
fn test_truncate() {
    let fs = format_and_mount();

    let file = fs
        .open("/trunc.txt", OpenFlags::WRITE | OpenFlags::CREATE)
        .unwrap();
    file.write(b"hello world").unwrap();
    file.close().unwrap();

    let file = fs.open("/trunc.txt", OpenFlags::WRITE).unwrap();
    file.truncate(5).unwrap();
    assert_eq!(file.size(), 5);
    file.close().unwrap();

    let data = fs.read_to_vec("/trunc.txt").unwrap();
    assert_eq!(data, b"hello");

    fs.unmount().unwrap();
}

#[test]
fn test_file_drop_closes() {
    let fs = format_and_mount();

    {
        let file = fs
            .open("/drop.txt", OpenFlags::WRITE | OpenFlags::CREATE)
            .unwrap();
        file.write(b"dropped").unwrap();
        // file dropped here without explicit close
    }

    let data = fs.read_to_vec("/drop.txt").unwrap();
    assert_eq!(data, b"dropped");

    fs.unmount().unwrap();
}

#[test]
fn test_open_nonexistent_fails() {
    let fs = format_and_mount();
    let result = fs.open("/nope.txt", OpenFlags::READ);
    assert!(result.is_err());
    assert_eq!(result.err().unwrap(), Error::NoEntry);
    fs.unmount().unwrap();
}

#[test]
fn test_mkdir_and_list() {
    let fs = format_and_mount();

    fs.mkdir("/docs").unwrap();
    fs.mkdir("/src").unwrap();

    let entries = fs.list_dir("/").unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"docs"));
    assert!(names.contains(&"src"));

    for entry in &entries {
        assert_eq!(entry.file_type, FileType::Dir);
    }

    fs.unmount().unwrap();
}

#[test]
fn test_remove_file() {
    let fs = format_and_mount();

    fs.write_file("/temp.txt", b"temp").unwrap();
    assert!(fs.exists("/temp.txt"));

    fs.remove("/temp.txt").unwrap();
    assert!(!fs.exists("/temp.txt"));

    fs.unmount().unwrap();
}

#[test]
fn test_remove_dir() {
    let fs = format_and_mount();

    fs.mkdir("/empty").unwrap();
    assert!(fs.exists("/empty"));

    fs.remove("/empty").unwrap();
    assert!(!fs.exists("/empty"));

    fs.unmount().unwrap();
}

#[test]
fn test_rename() {
    let fs = format_and_mount();

    fs.write_file("/old.txt", b"content").unwrap();
    fs.rename("/old.txt", "/new.txt").unwrap();

    assert!(!fs.exists("/old.txt"));
    let data = fs.read_to_vec("/new.txt").unwrap();
    assert_eq!(data, b"content");

    fs.unmount().unwrap();
}

#[test]
fn test_stat() {
    let fs = format_and_mount();

    fs.write_file("/info.txt", b"12345").unwrap();
    let meta = fs.stat("/info.txt").unwrap();
    assert_eq!(meta.file_type, FileType::File);
    assert_eq!(meta.size, 5);

    fs.mkdir("/subdir").unwrap();
    let meta = fs.stat("/subdir").unwrap();
    assert_eq!(meta.file_type, FileType::Dir);

    fs.unmount().unwrap();
}

#[test]
fn test_exists() {
    let fs = format_and_mount();
    assert!(!fs.exists("/nope"));
    fs.write_file("/yes.txt", b"y").unwrap();
    assert!(fs.exists("/yes.txt"));
    fs.unmount().unwrap();
}

#[test]
fn test_read_dir_iterator() {
    let fs = format_and_mount();

    fs.write_file("/a.txt", b"a").unwrap();
    fs.write_file("/b.txt", b"b").unwrap();
    fs.mkdir("/c").unwrap();

    let names: Vec<String> = {
        let dir = fs.read_dir("/").unwrap();
        let mut names = Vec::new();
        for entry in dir {
            names.push(entry.unwrap().name);
        }
        names
    };

    assert!(names.contains(&"a.txt".into()));
    assert!(names.contains(&"b.txt".into()));
    assert!(names.contains(&"c".into()));
    assert!(!names.contains(&".".into()));
    assert!(!names.contains(&"..".into()));

    fs.unmount().unwrap();
}

#[test]
fn test_read_dir_interleaved_with_file_ops() {
    let fs = format_and_mount();

    fs.write_file("/x.txt", b"data-x").unwrap();
    fs.write_file("/y.txt", b"data-y").unwrap();

    {
        let dir = fs.read_dir("/").unwrap();
        for entry in dir {
            let entry = entry.unwrap();
            if entry.file_type == FileType::File {
                let data = fs.read_to_vec(&format!("/{}", entry.name)).unwrap();
                assert!(!data.is_empty());
            }
        }
    }

    fs.unmount().unwrap();
}

#[test]
fn test_read_to_vec() {
    let fs = format_and_mount();

    fs.write_file("/hello.txt", b"Hello!").unwrap();
    let data = fs.read_to_vec("/hello.txt").unwrap();
    assert_eq!(data, b"Hello!");

    fs.unmount().unwrap();
}

#[test]
fn test_read_to_vec_empty() {
    let fs = format_and_mount();

    fs.write_file("/empty.txt", b"").unwrap();
    let data = fs.read_to_vec("/empty.txt").unwrap();
    assert!(data.is_empty());

    fs.unmount().unwrap();
}

#[test]
fn test_write_file_overwrites() {
    let fs = format_and_mount();

    fs.write_file("/f.txt", b"first").unwrap();
    fs.write_file("/f.txt", b"second").unwrap();
    let data = fs.read_to_vec("/f.txt").unwrap();
    assert_eq!(data, b"second");

    fs.unmount().unwrap();
}

#[test]
fn test_fs_size() {
    let fs = format_and_mount();
    let s1 = fs.fs_size().unwrap();
    assert!(s1 > 0);

    fs.write_file("/big.bin", &vec![0xAB; 4096]).unwrap();
    let s2 = fs.fs_size().unwrap();
    assert!(s2 > s1);

    fs.unmount().unwrap();
}

#[test]
fn test_nested_dirs() {
    let fs = format_and_mount();

    fs.mkdir("/a").unwrap();
    fs.mkdir("/a/b").unwrap();
    fs.write_file("/a/b/file.txt", b"nested").unwrap();

    let entries = fs.list_dir("/a/b").unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "file.txt");

    let data = fs.read_to_vec("/a/b/file.txt").unwrap();
    assert_eq!(data, b"nested");

    fs.unmount().unwrap();
}

#[test]
fn test_inline_rewrites_append_commits_instead_of_compacting() {
    let fs = format_and_mount_counting(4096, 64);
    let data = vec![0x5A; 350];

    for i in 0..200 {
        fs.write_file("/state.bin", &data)
            .unwrap_or_else(|e| panic!("rewrite {i} failed: {e:?}"));
    }
    assert_eq!(fs.read_to_vec("/state.bin").unwrap(), data);

    let storage = fs.unmount().expect("unmount");
    // Compacting on every commit would erase at least one block per rewrite.
    assert!(
        storage.erases < 50,
        "200 inline rewrites erased {} blocks; commits are not being appended",
        storage.erases
    );
}

#[test]
fn test_small_rewrites_fill_block_past_256_tags() {
    let fs = format_and_mount_counting(4096, 64);

    for i in 0u32..1000 {
        fs.write_file("/n", &i.to_le_bytes())
            .unwrap_or_else(|e| panic!("rewrite {i} failed: {e:?}"));
    }

    let storage = fs.unmount().expect("unmount");
    let fs = Filesystem::mount(storage, Config::new(4096, 64))
        .map_err(|(e, _)| e)
        .expect("remount");
    assert_eq!(fs.read_to_vec("/n").unwrap(), 999u32.to_le_bytes());
    fs.unmount().expect("unmount");
}

fn format_and_mount_counting(block_size: u32, block_count: u32) -> Filesystem<CountingStorage> {
    let mut storage = CountingStorage {
        inner: RamStorage::new(block_size, block_count),
        erases: 0,
    };
    let config = Config::new(block_size, block_count);
    Filesystem::format(&mut storage, &config).expect("format");
    storage.erases = 0;
    Filesystem::mount(storage, config)
        .map_err(|(e, _)| e)
        .expect("mount")
}

/// RamStorage wrapper that counts block erases.
struct CountingStorage {
    inner: RamStorage,
    erases: u32,
}

impl Storage for CountingStorage {
    fn read(&mut self, block: u32, offset: u32, buf: &mut [u8]) -> Result<(), Error> {
        self.inner.read(block, offset, buf)
    }

    fn write(&mut self, block: u32, offset: u32, data: &[u8]) -> Result<(), Error> {
        self.inner.write(block, offset, data)
    }

    fn erase(&mut self, block: u32) -> Result<(), Error> {
        self.erases += 1;
        self.inner.erase(block)
    }
}
