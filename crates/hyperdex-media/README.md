# hyperdex-media

Footage analysis for HyperDex: content fingerprints, stage 0 container
probing, the stage 1 sparse sampling pass (poster, filmstrip, coarse
scenes, black ranges, rough motion), the stage 2 full decode (exact
scenes, black ranges, motion scores, best moments, progress callbacks),
and the fingerprint keyed analysis cache. All codec work lives in
hyperdex-platform; this crate only consumes its plain data frames.

## Building against rsmpeg and FFmpeg

hyperdex-platform links FFmpeg as a library through the rsmpeg crate with
its `link_system_ffmpeg` feature, so building this crate needs a
FFmpeg development setup on the machine: headers, pkg-config files and
libclang for bindgen. A runtime only install is not enough. The
binding spike at `~/workspace/spikes/binding-spike/` (see its
RESULTS.md) proved rsmpeg 0.18.0 against FFmpeg 8.1.2 using a hand
built dev overlay, and this workspace builds against that overlay.
The production build is expected to pin and ship a full FFmpeg build
with its dev files instead.

Environment for cargo in this sandbox:

    RUSTUP_HOME=$HOME/workspace/.rustup
    CARGO_HOME=$HOME/workspace/.cargo
    PATH="$HOME/workspace/.cargo/bin:$PATH"
    PKG_CONFIG_PATH=$HOME/workspace/spikes/binding-spike/ffdev/lib/pkgconfig
    LIBCLANG_PATH=$HOME/workspace/spikes/binding-spike/pylibs/clang/native
    BINDGEN_EXTRA_CLANG_ARGS="-I/usr/lib/gcc/x86_64-linux-gnu/13/include"

What each piece does:

* `PKG_CONFIG_PATH` points pkg-config at the overlay, which carries
  the FFmpeg 8.1.2 headers and `.pc` files and resolves linking to
  the system runtime libraries.
* `LIBCLANG_PATH` points bindgen (through rusty_ffmpeg) at the
  libclang shared library it loads to parse the headers.
* `BINDGEN_EXTRA_CLANG_ARGS` adds the GCC builtin include directory
  that the header parse needs on this machine.

## Tests

The integration tests generate their fixtures with the system ffmpeg
(a moving box segment joined to a solid segment with a hard cut, and
a white, black, white clip), so ffmpeg must be on PATH to run them.
