# How does Tock compile?

There are two types of compilation artifacts in Tock: the kernel and user-level
processes (i.e. apps). Each type compiles differently. In addition, each
platform has a different way of programming the kernel and processes. Below is
an explanation of both kernel and process compilation as well as some examples
of how platforms program each onto an actual board.

<!-- npm i -g markdown-toc; markdown-toc -i Compilation.md -->

<!-- toc -->

- [Compiling the kernel](#compiling-the-kernel)
  * [Xargo](#xargo)
  * [Life of a Tock compilation](#life-of-a-tock-compilation)
- [Compiling a process](#compiling-a-process)
  * [Position Independent Code](#position-independent-code)
  * [Tock Binary Format](#tock-binary-format)
  * [Tock Application Bundle](#tock-application-bundle)
    + [TAB Format](#tab-format)
    + [Metadata](#metadata)
  * [Tock userland compilation environment](#tock-userland-compilation-environment)
    + [Compiling Libraries for Tock](#compiling-libraries-for-tock)
    + [Including other libraries](#including-other-libraries)
  * [Note for the Future](#note-for-the-future)
- [Loading the kernel and processes onto a board](#loading-the-kernel-and-processes-onto-a-board)

<!-- tocstop -->

## Compiling the kernel

The kernel is divided into five Rust crates (i.e. packages):

  * A core kernel crate containing key kernel operations such as handling
    interrupts and scheduling processes, shared kernel libraries such as
    `TakeCell`, and the Hardware Interface Layer (HIL) definitions. This is
    located in the `kernel/` folder.

  * An architecture (e.g. _ARM Cortex M4_) crate that implements context
    switching, and provides memory protection and systick drivers. This is
    located in the `arch/` folder.

  * A chip-specific (e.g. _Atmel SAM4L_) crate which handles interrupts and
    implements the hardware abstraction layer for a chip's peripherals. This is
    located in the `chips/` folder.

  * One (or more) crates for hardware independent drivers and virtualization
    layers. This is the `capsules/` folder in Tock. External projects using
    Tock may create additional crates for their own drivers.

  * A platform specific (e.g. _Imix_) crate that configures the chip and
    its peripherals, assigns peripherals to drivers, sets up virtualization
    layers, and defines a system call interface. This is located in `boards/`.

These crates are compiled using [Cargo](http://doc.crates.io), Rust's package
manager, with the platform crate as the base of the dependency graph. In
practice, the use of Cargo is masked by the Makefile system in Tock. Users can
simply type `make` from the proper directory in `boards/` to build the kernel
for that platform.

Internally, the Makefile is simply invoking Cargo to handle the build. For
example, `make` on the Imix platform translates to:

```bash
$ cargo build --release --target=sam4l.json
```

The `--release` argument tells Cargo to invoke the Rust compiler with
optimizations turned on and without debug symbols. `--target` points Cargo to
the target specification which includes the LLVM data-layout definition,
architecture definitions for the compiler, arguments to pass to the linker and
compilation options such as floating-point support.

### Xargo

While Cargo does manage building the Tock rust crates, Tock actually uses a
wrapper around Cargo called [Xargo](https://github.com/japaric/xargo). Xargo
is designed to help cross-compile the `core` crate provided by rust itself.
Once is has taken care of that cross-compilation, it passes through all commands
to Cargo proper.

In the future rust may incorporate support for building the core crates for ARM
targets directly, and we will no longer need Xargo.

### Life of a Tock compilation

When Cargo begins compiling the platform crate, it first resolves all
dependencies recursively. It chooses package versions that satisfy the
requirements across the dependency graph. Dependencies are defined in each
crate's `Cargo.toml` file and refer to paths in the local file-system, a
remote git repository, or a package published on [crates.io](http://crates.io).

Second, Cargo compiles each crate in turn as dependencies are satisfied. Each
crate is compiled as an `rlib` (an `ar` archive containing object files)
and combined into an executable ELF file by the compilation of the platform
crate.

You can see each command executed by `cargo` by passing it the `--verbose`
argument. In our build system, you can run `make V=1` to see the verbose
commands.


## Compiling a process

Unlike many other embedded systems, compilation of application code is entirely
separated from the kernel in Tock. An application is combined with at least two
libraries: `libtock` and `newlib` and built into a free-standing binary. The
binary can then be uploaded onto a Tock platform with an already existing
kernel to be loaded and run. For more details about application code, see
[Userland](./Userland.md).

Currently, all Tock platforms are ARM Cortex-M processors and all existing
applications are written in C. Therefore, compilation uses `arm-none-eabi-gcc`.
Alternative languages and compilers are all possible for building applications,
as long as they build code following several requirements:

 1. The application must be built as position independent code (PIC).

 2. The application must be linked with a loader script that places Flash
    contents above address `0x80000000` and RAM contents below it.

 3. The application binary must start with a header detailing the location of
    sections in the binary.

The first requirement is explained directly below while the second two are
detailed in [Tock Binary Format](#tock-binary-format). Again, as with the
kernel, the compilation process is handled by Makefiles and the user does not
normally need to interact with it.


### Position Independent Code

Since Tock loads applications separately from the kernel and is capable of
running multiple applications concurrently, applications cannot know in advance
at which address they will be loaded. This problem is common to many computer
systems and is typically addressed by dynamically linking and loading code at
runtime.

In Tock, however, we make a different choice and require applications to be
compiled as position independent code. Compiling with PIC makes all control
flow relative to the current PC, rather than using jumps to specified absolute
addresses. All data accesses are relative to the start of the data segment for
that app, and the address of the data segment is stored in a register referred
to as the `base register`. This potentially allows the segments in Flash and
RAM to be placed anywhere, and as long as the OS correctly initializes the base
register, everything will work fine.

PIC code can be inefficient on some architectures such as x86, but the ARM
instruction set is optimized for PIC operation and allows most code to execute
with little to no overhead. Using PIC still requires some fixup at runtime, but
the relocations are simple and cause only a one-time cost when an application
is loaded.  A more in-depth discussion of dynamically loading applications can
be found on the Tock website: [Dynamic Code Loading on a
MCU](http://www.tockos.org/blog/2016/dynamic-loading/).

For applications compiled with `arm-none-eabi-gcc`, building PIC code for Tock
requires four flags:

 - `-fPIC`: only emit code that uses relative addresses.
 - `-msingle-pic-base`: force the use of a consistent _base register_ for the
   data sections.
 - `-mpic-register=r9`: use register r9 as the base register.
 - `-mno-pic-data-is-text-relative`: do not assume that the data segment is
   placed at a constant offset from the text segment.


### Tock Binary Format

In order to be loaded correctly, applications must follow the Tock Binary
Format. This means the use of a linker script following specific rules and a
header for the binary so that Tock can load the application correctly.

Each Tock application uses a
[linker script](https://github.com/helena-project/tock/blob/master/userland/userland_generic.ld)
that places Flash at address `0x80000000` and SRAM at address `0x00000000`.
This allows relocations pointing at Flash to be easily differentiated from
relocations pointing at RAM.

Each Tock application begins with a header that is today defined as:

```rust
struct TbfHeader {
    version: u16,            // Version of the Tock Binary Format (currently 2)
    header_size: u16,        // Number of bytes in the complete TBF header
    total_size: u32,         // Total padded size of the program image in bytes, including header
    flags: u32,              // Various flags associated with the application
    checksum: u32,           // XOR of all 4 byte words in the header, including existing optional structs

    // Optional structs. All optional structs start on a 4-byte boundary.
    main: Option<TbfHeaderMain>,
    pic_options: Option<TbfHeaderPicOption1Fields>,
    name: Option<TbfHeaderPackageName>,
    flash_regions: Option<TbfHeaderWriteableFlashRegions>,
}

// Identifiers for the optional header structs.
enum TbfHeaderTypes {
    TbfHeaderMain = 1,
    TbfHeaderWriteableFlashRegions = 2,
    TbfHeaderPackageName = 3,
    TbfHeaderPicOption1 = 4,
}

// Type-length-value header to identify each struct.
struct TbfHeaderTlv {
    tipe: TbfHeaderTypes,    // 16 byte specifier of which struct follows
    length: u16,             // Number of bytes of the following struct
}

// Main settings required for all apps. If this does not exist, the "app" is
// considered padding and used to insert an empty linked-list element into the
// app flash space.
struct TbfHeaderMain {
    base: TbfHeaderTlv,
    init_fn_offset: u32,     // The function to call to start the application
    protected_size: u32,     // The number of bytes the application cannot write
    minimum_ram_size: u32,   // How much RAM the application is requesting
}

// Specifications for instructing the kernel to do PIC fixups for the application.
struct TbfHeaderPicOption1Fields {
    base: TbfHeaderTlv,
    text_offset: u32,            // Offset in memory to start of text segment
    data_offset: u32,            // Offset in memory to start of data
    data_size: u32,              // Length of data segment in bytes
    bss_memory_offset: u32,      // Offset in memory to start of BSS
    bss_size: u32,               // Length of BSS segment in bytes
    relocation_data_offset: u32, // Offset in memory to start of relocation data
    relocation_data_size: u32,   // Length of relocation data segment in bytes
    got_offset: u32,             // Offset in memory to start of GOT
    got_size: u32,               // Length of GOT segment in bytes
    minimum_stack_length: u32,   // Minimum stack size
}

// Optional package name for the app.
struct TbfHeaderPackageName {
    base: TbfHeaderTlv,
    package_name: [u8],      // UTF-8 string of the application name
}

// A defined flash region inside of the app's flash space.
struct TbfHeaderWriteableFlashRegion {
    writeable_flash_region_offset: u32,
    writeable_flash_region_size: u32,
}

// One or more specially identified flash regions the app intends to write.
struct TbfHeaderWriteableFlashRegions {
    base: TbfHeaderTlv,
    writeable_flash_regions: [TbfHeaderWriteableFlashRegion],
}
```

Flags:

```
   3                   2                   1                   0
 1 0 9 8 7 6 5 4 3 2 1 0 9 8 7 6 5 4 3 2 1 0 9 8 7 6 5 4 3 2 1 0
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
| Reserved                                                  |S|E|
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

- `E`: Enabled/disabled bit. When set to `1` the application will be started
on boot. When `0` the kernel will not start the application. Defaults to `1`
when set by `elf2tbf`.
- 'S': Sticky bit. When set to `1`, Tockloader will not remove the app without
a `--force` flag. This allows for "system" apps that can be added for debugging
purposes and are not removed during normal testing/application development.
The sticky bit also enables "library" applications (e.g. a radio stack) to
be persistent even when other apps are being developed.

In practice, this is automatically handled for applications. As part of the
compilation process, a tool called
[Elf to Tock Binary Format](https://github.com/helena-project/tock/tree/master/userland/tools/elf2tbf)
does the conversion from ELF to Tock's expected binary format, ensuring that
sections are placed in the expected order, adding a section that lists
necessary load-time relocations, and creating the TBF header.


### Tock Application Bundle

To support ease-of-use and distributable applications, Tock applications are
compiled for multiple architectures and bundled together into a "Tock
Application Bundle" or `.tab` file. This creates a standalone file for an
application that can be flashed onto any board that supports Tock, and removes
the need for the board to be specified when the application is compiled.
The TAB has enough information to be flashed on many or all Tock compatible
boards, and the correct binary is chosen when the application is flashed
and not when it is compiled.

#### TAB Format

`.tab` files are `tar`ed archives of TBF compatible binaries along with a
`metadata.toml` file that includes some extra information about the application.
A simplified example command that creates a `.tab` file is:

    tar cf app.tab cortex-m0.bin cortex-m4.bin metadata.toml

#### Metadata

The `metadata.toml` file in the `.tab` file is a TOML file that contains a
series of key-value pairs, one per line, that provides more detailed information
and can help when flashing the application. Existing fields:

```
tab-version = 1                         // TAB file format version
name = "<package name>"                 // Package name of the application
only-for-boards = <list of boards>      // Optional list of board kernels that this application supports
build-date = 2017-03-20T19:37:11Z       // When the application was compiled
```

### Tock userland compilation environment

Tock aims to provide a build environment that is easy for application authors
to integrate with. Check out the [examples](../userland/examples) folder for
sample applications. The Tock userland build system will automatically build
with all of the correct flags and generate TABs for all supported Tock
architectures.

To leverage the Tock build system, you must:

  1. Set `TOCK_USERLAND_BASE_DIR` to the path to the Tock userland.
  2. `include $(TOCK_USERLAND_BASE_DIR)/AppMakefile.mk`.

This `include` should be the _last_ line of the Makefile for most applications.

In addition, you must specify the sources for your application:

  - `C_SRCS`: A list of C files to compile.
  - `CXX_SRCS`: A list of C++ files to compile.
  - `AS_SRCS`: A list of assembly files to compile.
  - `EXTERN_LIBS`: A list of directories for libraries [**compiled for Tock**](#compiling-libraries-for-tock).

The build system respects all of the standard `CFLAGS` (C only), `CXXFLAGS`
(C++ only), `CPPFLAGS` (C and C++), `ASFLAGS` (asm only), etc.

Several Tock-specific variables are also useful:

  - `STACK_SIZE`: The minimum application stack size.
  - `APP_HEAP_SIZE`: The minimum heap size for your application.
  - `KERNEL_HEAP_SIZE`: The minimum grant size for your application.
  - `PACKAGE_NAME`: The name for your application. Defaults to current folder.

The build system is broken across three files in the `tock/userland` folder:

  - `Configuration.mk`: Sets most variables used.
  - `Helpers.mk`: Generic rules and functions to support the build.
  - `AppMakefile.mk`: Includes the above files and supplies build recipes.

Applications wishing to define their own build rules can include only the
`Configuration.mk` file to ensure all of the flags needed for Tock applications
are included.

#### Compiling Libraries for Tock

Libraries used by Tock need all of the same position-independent build flags as
the final application. As Tock builds for all supported architectures by
default, libraries should include images for each supported Tock architecture.

To leverage the `EXTERN_LIBS` variable, external libraries must adhere to the
following structure:

```
For the library "example"

libexample/                <-- Folder name must match library name
├── Makefile.app           <-- Optional additional rules to include when building apps
├── build
│   ├── cortex-m0          <-- Architecture names match gcc's -mcpu= flag
│   │   └── libexample.a   <-- Library name must match folder name
│   └── cortex-m4
│       └── libexample.a   <-- Library name must match folder name
└── include                <-- Optional include/ directory will be added to include path
    └── example.h
```

Like applications, libraries can leverage the Tock build system to do most of
the heavy lifting. Simply,

  1. Set `TOCK_USERLAND_BASE_DIR` to the path to the Tock userland.
  2. `include $(TOCK_USERLAND_BASE_DIR)/TockLibrary.mk`.

and add sources using the same variables as applications.

#### Including other libraries

To manually include an external library, add the library to each `LIBS_$(arch)`
(i.e. `LIBS_cortex-m0`) variable. You can include header paths using the
standard search mechanisms (i.e. `CPPFLAGS += -I<path>`).

### Note for the Future

All these requirements exist in current Tock, but are not fundamental. Future
version of Tock may support dynamic runtime application linking and loading.


## Loading the kernel and processes onto a board

There is no particular limitation on how code can be loaded onto a board. JTAG
and various bootloaders are all equally possible. Currently, the `hail`
platform uses either JTAG or a serial bootloader, the `imix` platform supports
JTAG, and the `nrf51dk` platform supports the mbed bootloader which presents
itself as a USB storage device that `.bin` files can be copied into. All of
these methods are subject to change based on whatever is easiest for users of
the platform.

In order to support multiple concurrent applications, the easiest option is to
use `tockloader` ([git repo](https://github.com/helena-project/tockloader)) to
manage multiple applications on a platform. Importantly, while applications
currently share the same upload process as the kernel, they are planned to
support additional methods in the future. Application loading through wireless
methods especially is targeted for future editions of Tock.
