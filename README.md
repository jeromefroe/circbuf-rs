An implementation of a growable circular buffer of bytes. The `CircBuf` struct
manages a buffer of bytes allocated on the heap. The buffer can be grown when needed
and can return slices into its internal buffer that can be used for both normal IO
(`read` and `write`) as well as vector IO (`readv` and `writev`).

## Example

Below is a simple example of a server which makes use of a `CircBuf` to read messages
from a client. It uses the `vecio` crate to call `readv` and `writev` on the socket.
Messages are seperated by a vertical bar '|' and the server returns to the client
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