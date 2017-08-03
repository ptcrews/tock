Tock Getting Started Guide
==========================

This covers how to get the toolchain setup on your platform to start using and
developing Tock.

## Requirements

1. [Rust](http://www.rust-lang.org/) (install `rustup` so Tock will choose the right version automatically)
1. [Xargo](http://www.rust-lang.org/) (Rust `cargo` wrapper that installs core library for embedded targets)
2. [arm-none-eabi toolchain](https://developer.arm.com/open-source/gnu-toolchain/gnu-rm/downloads) (version >= 5.2)
3. Command line utilities: wget, sed, make, cmake

### Installing Requirements

#### Rust (nightly)

We are using `rustc 1.19.0-nightly (04145943a 2017-06-19)`. We recommend
installing it with [rustup](http://www.rustup.rs) so you can manage multiple
versions of Rust and continue using stable versions for other Rust code:

```bash
$ curl https://sh.rustup.rs -sSf | sh
```

This will install `rustup` in your home directory, so you will need to
source `~/.profile` or open a new shell to add the `.cargo/bin` directory
to your `$PATH`.

Then install the correct nightly version of Rust:

```bash
$ rustup install nightly-2017-06-20
```

#### Xargo

Rust core libraries for ARM Cortex-M target do not come with `rustup` by
default, so we use [`xargo`](https://github.com/japaric/xargo), a wrapper
around `cargo`, which compiles these libraries.

```bash
$ cargo install xargo
```

#### `arm-none-eabi` toolchain

We generally track the latest version of arm-none-eabi-gcc [as released by
ARM](https://developer.arm.com/open-source/gnu-toolchain/gnu-rm/downloads).

There are known issues with arm-none-eabi-gcc version 5.1 and older, or other
versions packaged with a newlib version earlier than 2.3, as they will run into
problems with missing ARM intrinsics (e.g., `__aeabi_memclr`). Tock does not
support these versions.

##### Compiled Binaries

Pre-compiled binaries are available [from
ARM](https://developer.arm.com/open-source/gnu-toolchain/gnu-rm/downloads).
The recommendations below will set up your operating system's package manager
to track the latest release from ARM.

##### MacOS

With [Homebrew](http://brew.sh/) (preferred):

```bash
$ brew tap ARMmbed/homebrew-formulae
$ brew update
$ brew install arm-none-eabi-gcc
```

or with [MacPorts](https://www.macports.org/):

```bash
$ port install arm-none-eabi-gcc
```

###### Heads Up!

The `make debug` target asks the Tock build system to generate a listings
(disassembly) file. Some developers have noticed that `arm-none-eabi-objdump`
takes a long time (order several minutes) on a mac while Activity Monitor
reports that `opendirectoryd` pegs the CPU.

This is a [known issue](http://superuser.com/questions/350879/) that you can
resolve by commenting out the `/home` line from `/etc/auto_master` and then
running `sudo automount -vc` to apply the changes.

##### Linux

If you install the binaries but get a "no such file or directory" error
when trying to run them, then you are most likely missing needed libraries.
Check that you have a 64-bit version of libc installed.

###### Ubuntu

```bash
$ sudo add-apt-repository ppa:team-gcc-arm-embedded/ppa
$ sudo apt-get update
$ sudo apt-get install gcc-arm-embedded
```

###### Arch

On Arch Linux the `arm-none-eabi-newlib` package in pacman contains a
sufficiently up-to-date version of newlibc.

```bash
$ sudo pacman -S arm-none-eabi-gcc arm-none-eabi-newlib arm-none-eabi-gdb
```

##### Windows

You can download precompiled binaries for Windows from the ARM site listed
above. While we expect things should work on Windows, none of the active Tock
developers currently develop on Windows, so it is possible that are some
unexpected pitfalls.

##### Other

Alternatively, if you would like simulator mode in `arm-none-eabi-gdb`,
you can use the build scripts in the `tools` directory, in this order:
`build-arm-binutils` then `build-arm-gcc` then `build-arm-gdb`.

## Compiling the Kernel

To build the kernel, just type `make` in the root directory.  The root
Makefile selects a board and architecture to build the kernel for and
routes all calls to that board's specific Makefile. The root Makefile
is set up with the following defaults:

```
TOCK_BOARD ?= hail
```

Thus it compiles for the Hail board by default. There are two ways to
build for a different board:

 * You can compile the kernel for a specific board by running the command
   from inside the board's directory:

    ```bash
    $ cd boards/nrf51dk/
    $ make
    ```

 * Alternatively, you can add a `TOCK_BOARD` environment variable where
    `TOCK_BOARD` is the directory name inside `boards/`.

    ```bash
    $ make TOCK_BOARD=nrf51dk
    ```

Board specific Makefiles are located in `boards/<BOARD>/`. Some boards have
special build options that can only be used within the board's directory.
Generic options such as `clean`, `doc`, `debug`, `program`, and `flash` can be
accessed from Tock's root.

## Compiling applications

All user-level code lives in the `userland` subdirectory. This
includes a specially compiled version of newlib, a user-level library
for talking to the kernel and specific drivers and a variety of
example applications.

Compiled applications are architecture-specific (e.g. `cortex-m4`,
`cortex-m0`) since the compiler emits slightly different instructions
for each variant. Compiled applications can also depend on specific
drivers, which not all boards provide; if you load an application onto
a board that does not support every driver/system call it uses, some
system calls with return error codes (`ENODEVICE` or `ENOSUPPORT`).

Applications are built for all architectures Tock supports, currently
`cortex-m0` and `cortex-m4`. Boards select an appropriate architecture when
uploading code (e.g. `cortex-m4` for the SAM4L on the `imix` board).

To compile an app, `cd` to the desired app and `make`. For example:

```bash
$ cd userland/examples/blink/
$ make
```

This will build the app and generate a binary in Tock Binary Format
(using the `elf2tbf` utility):
`userland/examples/blink/build/cortex-m4/cortex-m4.bin`.

Alternatively, apps can be built and automatically uploaded from the
Tock root directory:

```bash
$ make examples/blink
```

## Loading the kernel and applications onto a board

### Optional Requirements

For some boards, currently `Hail` and `imix` (but not `imixv1`), you will need
`tockloader`. `tockloader` also has features that are generally useful to all
Tock boards, such as easy to manage serial connections, and the ability to
list, add, replace, and remove applications over JTAG (or USB if a bootloader
is installed).

1. [tockloader](https://github.com/helena-project/tockloader) (version 0.7.1)

Installing applications over JTAG, depending on your JTAG Debugger, you will
need one of:

1. [openocd](http://openocd.org/) (version >= 0.8.0)
2. [JLinkExe](https://www.segger.com/downloads/jlink) (version >= 5.0)

#### `tockloader`

Tock requires `tockloader` version `0.7.1`. To install:

```bash
(Linux): sudo pip3 install tockloader==0.7.1
(MacOS): pip3 install tockloader==0.7.1
```

#### `openocd`

Works with various JTAG debuggers. We require at least version `0.8.0` to
support the SAM4L on `imix`.

```bash
(Linux): sudo apt-get install openocd
(MacOS): brew install open-ocd
```

#### `JLinkExe`

If you want to upload code through a [JLink JTAG
debugger](https://www.segger.com/j-link-edu.html) (available on
[Digikey](https://www.digikey.com/product-detail/en/segger-microcontroller-systems/8.08.90-J-LINK-EDU/899-1008-ND/2263130)), you should install JLinkExe. We require a version greater than or equal to `5.0`.

It is available [here](https://www.segger.com/downloads/jlink). You want to the
"J-Link Software and Documentation Pack". There are various packages available
depending on operating system.

### Loading code onto a board

This is generally done with `make program` and `make flash`, but is board
specific. To learn how to program your specific hardware, please see
the board specific READMEs:

* [imix](../boards/imix/README.md)
* [Hail](../boards/hail/README.md)
* [nRF](../boards/nrf51dk/README.md)


## Formatting Rust Source Code

Rust includes a tool for automatically formatting Rust source
code. This requires a `cargo` tool:

    $ cargo install rustfmt

Then run:

    $ make format

to format the repository.
