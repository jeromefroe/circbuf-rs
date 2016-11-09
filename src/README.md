An implementation of a growable circular buffer of bytes. The `CircBuf` struct
manages a buffer of bytes allocated on the heap. The buffer can be grown when needed
and can return slices into its internal buffer that can be used for both normal IO
(e.g. `read` and `write`) as well as vector IO (`readv` and `writev`).