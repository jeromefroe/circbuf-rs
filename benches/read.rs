#![feature(can_vector)]
#![feature(test)]
extern crate test;

use circbuf::CircBuf;
use test::Bencher;

use std::{
    fs::File,
    io::{copy, IoSliceMut, Read, Seek, Write},
};

const EXPECTED_N: usize = 12;

fn prepare_file() -> File {
    let mut file = tempfile::tempfile().unwrap();
    assert_eq!(file.write(b"foo\nbar\nbaz\n").unwrap(), EXPECTED_N);
    file
}

fn prepare_circbuf() -> CircBuf {
    let mut c = CircBuf::with_capacity(16).unwrap();
    c.advance_read(8);
    c.advance_write(8);
    c
}

#[bench]
fn normal_read(b: &mut Bencher) {
    let mut file = prepare_file();
    let mut c = prepare_circbuf();

    b.iter(|| {
        file.rewind().unwrap();
        let [mut a, mut b] = c.get_avail();
        let n_a = file.read(&mut a).unwrap();
        let n_b = file.read(&mut b).unwrap();
        let n = n_a + n_b;
        assert_eq!(n, EXPECTED_N);

        c.advance_write(n);
        c.advance_read(n);
    })
}

#[bench]
fn writer_read(b: &mut Bencher) {
    let mut file = prepare_file();
    let mut c = prepare_circbuf();

    b.iter(|| {
        file.rewind().unwrap();
        let n = copy(&mut file, &mut c).unwrap() as usize;
        assert_eq!(n, EXPECTED_N);
        c.advance_read(n);
    })
}

#[bench]
#[ignore = "Test need OS with vectored read feature"]
fn vector_read(b: &mut Bencher) {
    let mut file = prepare_file();
    assert!(file.is_read_vectored());
    let mut c = prepare_circbuf();

    b.iter(|| {
        file.rewind().unwrap();
        let mut bufs = {
            let [a, b] = c.get_avail();
            [IoSliceMut::new(a), IoSliceMut::new(b)]
        };
        let n = file.read_vectored(bufs.as_mut_slice()).unwrap();
        assert_eq!(n, EXPECTED_N);
        c.advance_write(n);
        c.advance_read(n);
    })
}

#[bench]
// we call seek in iter in both benches above so lets get a base time for it
fn seek_base(b: &mut Bencher) {
    let mut file = prepare_file();

    b.iter(|| {
        file.rewind().unwrap();
    })
}
