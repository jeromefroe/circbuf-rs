# CircBuf

[![Build Status](https://travis-ci.org/jeromefroe/circbuf-rs.svg?branch=master)](https://travis-ci.org/jeromefroe/circbuf-rs)
[![Coverage Status](https://coveralls.io/repos/github/jeromefroe/circbuf-rs/badge.svg?branch=master)](https://coveralls.io/github/jeromefroe/circbuf-rs?branch=master)
[![crates.io](https://img.shields.io/crates/v/circbuf.svg)](https://crates.io/crates/circbuf/)
[![docs.rs](https://docs.rs/circbuf/badge.svg)](https://docs.rs/circbuf/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://raw.githubusercontent.com/jeromefroe/circbuf-rs/master/LICENSE)

[Documentation](https://docs.rs/circbuf/)

An implementation of a growable circular buffer of bytes. The `CircBuf` struct
manages a buffer of bytes allocated on the heap. The buffer can be grown when needed
and can return slices into its internal buffer that can be used for both normal IO
(`read` and `write`) as well as vector IO (`readv` and `writev`).

## Example

Below is a simple example of a server which makes use of a `CircBuf` to read messages
from a client. It uses the `vecio` crate to call `readv` and `writev` on the socket.
Messages are seperated by a vertical bar `|` and the server returns to the client
the number of bytes in each message it receives.

```rust
extern crate vecio;
extern crate circbuf;

use std::thread;
use std::net::{TcpListener, TcpStream};
use std::io::Write;
use vecio::Rawv;
use circbuf::CircBuf;

fn handle_client(mut stream: TcpStream) {
    let mut buf = CircBuf::new();
    let mut num_messages = 0; // number of messages from the client
    let mut num_bytes = 0; // number of bytes read since last '|'

    loop {
        // grow the buffer if it is less than half full
        if buf.len() > buf.avail() {
            buf.grow().unwrap();
        }

        let n;
        {
            n = match stream.readv(&buf.get_avail()) {
                Ok(n) => {
                    if n == 0 {
                        // EOF
                        println!("client closed connection");
                        break;
                    }
                    n
                }
                Err(e) => panic!("got an error reading from a connection: {}", e),
            };
        }

        println!("read {} bytes from the client", n);

        // update write cursor
        buf.advance_write(n);

        // parse request from client for messages seperated by '|'
        loop {
            match buf.find_from_index(b'|', num_bytes) {
                Some(i) => {
                    // update read cursor past '|' and reset num_bytes since last '|'
                    buf.advance_read(i + 1);
                    num_bytes = 0;
                    num_messages += 1;

                    let response = format!("Message {} contained {} bytes\n", num_messages, i - 1); // don't inclue '|' in num_bytes
                    match stream.write(&response.as_bytes()) {
                        Ok(n) => {
                            println!("wrote {} bytes to the client", n);
                        }
                        Err(e) => panic!("got an error writing to connection: {}", e),
                    }
                }
                None => break,
            }
        }
    }
}

fn main() {
    let listener = TcpListener::bind("127.0.0.1:8888").unwrap();
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                thread::spawn(move || handle_client(stream));
            }
            Err(e) => panic!("got an error accepting connection: {}", e),
        }
    }
}
```