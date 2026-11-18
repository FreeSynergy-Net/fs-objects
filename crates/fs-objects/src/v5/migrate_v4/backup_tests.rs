//! Pattern: Unit tests of the checksum and copy helpers (block reading, copy check).

use std::fmt::Write as _;
use std::io::Read;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

use super::{BLOCK, feed, fingerprint, verify_copy};

fn folder(name: &str) -> PathBuf {
    // Beside the test binary (under the target folder, never /tmp, which is RAM).
    let exe = std::env::current_exe().unwrap();
    let d = exe
        .parent()
        .unwrap()
        .join(format!("bk-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn data(n: usize) -> Vec<u8> {
    (0..n).map(|i| u8::try_from(i * 7 % 251).unwrap()).collect()
}

/// Remembers the largest buffer it was asked to fill.
struct Spy {
    left: Vec<u8>,
    biggest: usize,
}

impl Read for Spy {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.biggest = self.biggest.max(buf.len());
        let n = buf.len().min(self.left.len());
        buf[..n].copy_from_slice(&self.left[..n]);
        self.left.drain(..n);
        Ok(n)
    }
}

#[test]
fn the_hash_is_fed_in_blocks_and_equals_the_whole_hash() {
    let bytes = data(BLOCK * 3 + 7);
    let mut spy = Spy {
        left: bytes.clone(),
        biggest: 0,
    };
    let mut h = Sha256::new();
    feed(&mut spy, &mut h).unwrap();
    assert_eq!(
        spy.biggest, BLOCK,
        "no read is bigger than one block: memory does not grow with the file"
    );
    assert_eq!(h.finalize().to_vec(), Sha256::digest(&bytes).to_vec());
}

#[test]
fn the_fingerprint_of_a_file_larger_than_a_block_is_the_old_hash_over_file_and_wal() {
    let d = folder("fp");
    let (file, wal) = (d.join("x.db"), d.join("x.db-wal"));
    let (a, b) = (data(BLOCK * 2 + 5), data(BLOCK + 11));
    std::fs::write(&file, &a).unwrap();
    std::fs::write(&wal, &b).unwrap();
    let mut h = Sha256::new();
    h.update(&a);
    h.update(b"\0wal\0");
    h.update(&b);
    let want = h.finalize().iter().fold(String::new(), |mut a, x| {
        let _ = write!(a, "{x:02x}");
        a
    });
    assert_eq!(fingerprint(&file).unwrap(), want);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_copy_that_differs_is_refused_and_an_equal_one_passes() {
    let d = folder("vc");
    let (a, b) = (d.join("a"), d.join("b"));
    std::fs::write(&a, data(1000)).unwrap();
    std::fs::write(&b, data(1000)).unwrap();
    verify_copy(&a, &b).unwrap();
    let mut other = data(1000);
    other[500] ^= 1;
    std::fs::write(&b, other).unwrap();
    assert!(
        verify_copy(&a, &b).is_err(),
        "a flipped bit in the copy must be found"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(unix)]
#[test]
fn a_file_that_was_replaced_is_read_again_and_one_that_was_changed_in_place_too() {
    let d = folder("rep");
    let file = d.join("x.db");
    std::fs::write(&file, data(100)).unwrap();
    let first = fingerprint(&file).unwrap();
    std::fs::write(&file, data(101)).unwrap();
    let in_place = fingerprint(&file).unwrap();
    assert_ne!(first, in_place, "a change in place must show");
    std::fs::remove_file(&file).unwrap();
    std::fs::write(&file, data(102)).unwrap();
    let replaced = fingerprint(&file).unwrap();
    assert_ne!(
        in_place, replaced,
        "a replaced file (new inode) must be read, not the old one"
    );
    let _ = std::fs::remove_dir_all(&d);
}
