//! Support for creating and running userspace applications.

use callback::AppId;
use common::cells::VolatileCell;
use common::{Queue, RingBuffer};

use core::cell::Cell;
use core::fmt::Write;
use core::ptr::{read_volatile, write, write_volatile};
use core::{mem, ptr, slice, str};
use grant;

use common::math;
use platform::mpu;
use returncode::ReturnCode;
use syscall::Syscall;

/// Takes a value and rounds it up to be aligned % 8
macro_rules! align8 {
    ($e:expr) => {
        ($e) + ((8 - (($e) % 8)) % 8)
    };
}

/// Takes a value and rounds it up to be aligned % 4
macro_rules! align4 {
    ($e:expr) => {
        ($e) + ((4 - (($e) % 4)) % 4)
    };
}

/// This is used in the hardfault handler.
#[allow(private_no_mangle_statics)]
#[no_mangle]
static mut SYSCALL_FIRED: usize = 0;

/// This is used in the hardfault handler.
#[allow(private_no_mangle_statics)]
#[no_mangle]
static mut APP_FAULT: usize = 0;

/// This is used in the hardfault handler.
#[allow(private_no_mangle_statics)]
#[no_mangle]
static mut SCB_REGISTERS: [u32; 5] = [0; 5];

#[allow(improper_ctypes)]
extern "C" {
    pub fn switch_to_user(user_stack: *const u8, process_regs: &mut [usize; 8]) -> *mut u8;
}

pub static mut PROCS: &'static mut [Option<&mut Process<'static>>] = &mut [];

/// Helper function to load processes from flash into an array of active
/// processes. This is the default template for loading processes, but a board
/// is able to create its own `load_processes()` function and use that instead.
///
/// Processes are found in flash starting from the given address and iterating
/// through Tock Binary Format headers. Processes are given memory out of the
/// `app_memory` buffer until either the memory is exhausted or the allocated
/// number of processes are created, with process structures placed in the
/// provided array. How process faults are handled by the kernel is also
/// selected.
pub unsafe fn load_processes(
    start_of_flash: *const u8,
    app_memory: &mut [u8],
    procs: &mut [Option<&mut Process<'static>>],
    fault_response: FaultResponse,
) {
    let mut apps_in_flash_ptr = start_of_flash;
    let mut app_memory_ptr = app_memory.as_mut_ptr();
    let mut app_memory_size = app_memory.len();
    for i in 0..procs.len() {
        let (process, flash_offset, memory_offset) = Process::create(
            apps_in_flash_ptr,
            app_memory_ptr,
            app_memory_size,
            fault_response,
        );

        if process.is_none() {
            // We did not get a valid process, but we may have gotten a disabled
            // process or padding. Therefore we want to skip this chunk of flash
            // and see if there is a valid app there. However, if we cannot
            // advance the flash pointer, then we are done.
            if flash_offset == 0 && memory_offset == 0 {
                break;
            }
        } else {
            procs[i] = process;
        }

        apps_in_flash_ptr = apps_in_flash_ptr.offset(flash_offset as isize);
        app_memory_ptr = app_memory_ptr.offset(memory_offset as isize);
        app_memory_size -= memory_offset;
    }
}

pub fn schedule(callback: FunctionCall, appid: AppId) -> bool {
    let procs = unsafe { &mut PROCS };
    let idx = appid.idx();
    if idx >= procs.len() {
        return false;
    }

    match procs[idx] {
        None => false,
        Some(ref mut p) => {
            // If this app is in the `Fault` state then we shouldn't schedule
            // any work for it.
            if p.current_state() == State::Fault {
                return false;
            }

            unsafe {
                HAVE_WORK.set(HAVE_WORK.get() + 1);
            }

            let ret = p.tasks.enqueue(Task::FunctionCall(callback));

            // Make a note that we lost this callback if the enqueue function
            // fails.
            if ret == false {
                p.debug
                    .dropped_callback_count
                    .set(p.debug.dropped_callback_count.get() + 1);
            }

            ret
        }
    }
}

/// Returns the full address of the start and end of the flash region that the
/// app owns and can write to. This includes the app's code and data and any
/// padding at the end of the app. It does not include the TBF header, or any
/// space that the kernel is using for any potential bookkeeping.
pub fn get_editable_flash_range(app_idx: usize) -> (usize, usize) {
    let procs = unsafe { &mut PROCS };
    if app_idx >= procs.len() {
        return (0, 0);
    }

    match procs[app_idx] {
        None => (0, 0),
        Some(ref mut p) => {
            let start = p.flash_non_protected_start() as usize;
            let end = p.flash_end() as usize;
            (start, end)
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Error {
    NoSuchApp,
    OutOfMemory,
    AddressOutOfBounds,
}

impl From<Error> for ReturnCode {
    fn from(err: Error) -> ReturnCode {
        match err {
            Error::OutOfMemory => ReturnCode::ENOMEM,
            Error::AddressOutOfBounds => ReturnCode::EINVAL,
            Error::NoSuchApp => ReturnCode::EINVAL,
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum State {
    Running,
    Yielded,
    Fault,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum FaultResponse {
    Panic,
    Restart,
}

#[derive(Copy, Clone, Debug)]
pub enum IPCType {
    Service,
    Client,
}

#[derive(Copy, Clone, Debug)]
pub enum Task {
    FunctionCall(FunctionCall),
    IPC((AppId, IPCType)),
}

#[derive(Copy, Clone, Debug)]
pub struct FunctionCall {
    pub r0: usize,
    pub r1: usize,
    pub r2: usize,
    pub r3: usize,
    pub pc: usize,
}

/// Legacy Tock Binary Format header.
///
/// Version 1 of the header is deprecated but can still be parsed by the kernel
/// to support any apps that were compiled with an older version of elf2tbf.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct TbfHeaderV1 {
    version: u32,
    total_size: u32,
    entry_offset: u32,
    rel_data_offset: u32,
    rel_data_size: u32,
    text_offset: u32,
    text_size: u32,
    got_offset: u32,
    got_size: u32,
    data_offset: u32,
    data_size: u32,
    bss_mem_offset: u32,
    bss_size: u32,
    min_stack_len: u32,
    min_app_heap_len: u32,
    min_kernel_heap_len: u32,
    pkg_name_offset: u32,
    pkg_name_size: u32,
    checksum: u32,
}

/// TBF fields that must be present in all v2 headers.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct TbfHeaderV2Base {
    version: u16,
    header_size: u16,
    total_size: u32,
    flags: u32,
    checksum: u32,
}

/// Types in TLV structures for each optional block of the header.
#[repr(u16)]
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
enum TbfHeaderTypes {
    TbfHeaderMain = 1,
    TbfHeaderWriteableFlashRegions = 2,
    TbfHeaderPackageName = 3,
    Unused = 5,
}

/// The TLV header (T and L).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct TbfHeaderTlv {
    tipe: TbfHeaderTypes,
    length: u16,
}

/// The v2 main section for apps.
///
/// All apps must have a main section. Without it, the header is considered as
/// only padding.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct TbfHeaderV2Main {
    init_fn_offset: u32,
    protected_size: u32,
    minimum_ram_size: u32,
}

/// Writeable flash regions only need an offset and size.
///
/// There can be multiple (or zero) flash regions defined, so this is its own
/// struct.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct TbfHeaderV2WriteableFlashRegion {
    writeable_flash_region_offset: u32,
    writeable_flash_region_size: u32,
}

/// PIC fields for kernel provided PIC fixup.
///
/// If an app wants the kernel to do the PIC fixup for it, it must pass this
/// block so the kernel knows where sections are in the app binary.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct PicOption1Fields {
    text_offset: u32,
    data_offset: u32,
    data_size: u32,
    bss_memory_offset: u32,
    bss_size: u32,
    relocation_data_offset: u32,
    relocation_data_size: u32,
    got_offset: u32,
    got_size: u32,
    minimum_stack_length: u32,
}

/// Single header that can contain all parts of a v2 header.
#[derive(Clone, Copy, Debug)]
struct TbfHeaderV2 {
    base: &'static TbfHeaderV2Base,
    main: Option<&'static TbfHeaderV2Main>,
    package_name: Option<&'static str>,
    writeable_regions: Option<&'static [TbfHeaderV2WriteableFlashRegion]>,
}

/// Type that represents the fields of the Tock Binary Format header.
///
/// This specifies the locations of the different code and memory sections
/// in the tock binary, as well as other information about the application.
/// The kernel can also use this header to keep persistent state about
/// the application.
#[derive(Debug)]
enum TbfHeader {
    TbfHeaderV1(&'static TbfHeaderV1),
    TbfHeaderV2(TbfHeaderV2),
    Padding(&'static TbfHeaderV2Base),
}

impl TbfHeader {
    /// Return whether this is an app or just padding between apps.
    fn is_app(&self) -> bool {
        match *self {
            TbfHeader::TbfHeaderV1(_) => true,
            TbfHeader::TbfHeaderV2(_) => true,
            TbfHeader::Padding(_) => false,
        }
    }

    /// Return whether the application is enabled or not.
    /// Disabled applications are not started by the kernel.
    fn enabled(&self) -> bool {
        match *self {
            // Header v1 has no flag for this, and therefore all apps are
            // always enabled.
            TbfHeader::TbfHeaderV1(_) => true,
            TbfHeader::TbfHeaderV2(hd) => {
                // Bit 1 of flags is the enable/disable bit.
                hd.base.flags & 0x00000001 == 1
            }
            TbfHeader::Padding(_) => false,
        }
    }

    /// Get the total size in flash of this app or padding.
    fn get_total_size(&self) -> u32 {
        match *self {
            TbfHeader::TbfHeaderV1(hd) => hd.total_size,
            TbfHeader::TbfHeaderV2(hd) => hd.base.total_size,
            TbfHeader::Padding(hd) => hd.total_size,
        }
    }

    /// Add up all of the relevant fields in header version 1, or just used the
    /// app provided value in version 2 to get the total amount of RAM that is
    /// needed for this app.
    fn get_minimum_app_ram_size(&self) -> u32 {
        match *self {
            TbfHeader::TbfHeaderV1(hd) => {
                let heap_len = align8!(hd.min_app_heap_len) + align8!(hd.min_kernel_heap_len);
                let data_len = hd.data_size + hd.got_size + hd.bss_size;
                let stack_size = align8!(hd.min_stack_len);
                align8!(data_len + stack_size) + heap_len
            }
            TbfHeader::TbfHeaderV2(hd) => hd.main.map_or(0, |m| m.minimum_ram_size),
            _ => 0,
        }
    }

    /// Get the number of bytes from the start of the app's region in flash that
    /// is for kernel use only. The app cannot write this region.
    fn get_protected_size(&self) -> u32 {
        match *self {
            TbfHeader::TbfHeaderV1(_) => mem::size_of::<TbfHeaderV1>() as u32,
            TbfHeader::TbfHeaderV2(hd) => {
                hd.main.map_or(0, |m| m.protected_size) + (hd.base.header_size as u32)
            }
            _ => 0,
        }
    }

    /// Get the offset from the beginning of the app's flash region where the
    /// app should start executing.
    fn get_init_function_offset(&self) -> u32 {
        match *self {
            TbfHeader::TbfHeaderV1(hd) => hd.entry_offset,
            TbfHeader::TbfHeaderV2(hd) => {
                hd.main.map_or(0, |m| m.init_fn_offset) + (hd.base.header_size as u32)
            }
            _ => 0,
        }
    }

    /// Get the name of the app.
    fn get_package_name(&self, flash_start_addr: *const u8) -> &'static str {
        match *self {
            TbfHeader::TbfHeaderV1(hd) => unsafe {
                let package_name_byte_array = slice::from_raw_parts(
                    flash_start_addr.offset(hd.pkg_name_offset as isize),
                    hd.pkg_name_size as usize,
                );
                let mut app_name_str = "";
                let _ = str::from_utf8(package_name_byte_array).map(|name_str| {
                    app_name_str = name_str;
                });
                app_name_str
            },
            TbfHeader::TbfHeaderV2(hd) => hd.package_name.unwrap_or(""),
            _ => "",
        }
    }

    /// Get the number of flash regions this app has specified in its header.
    fn number_writeable_flash_regions(&self) -> usize {
        match *self {
            TbfHeader::TbfHeaderV1(_) => 0,
            TbfHeader::TbfHeaderV2(hd) => hd.writeable_regions.map_or(0, |wr| wr.len()),
            _ => 0,
        }
    }

    /// Get the offset and size of a given flash region.
    fn get_writeable_flash_region(&self, index: usize) -> (u32, u32) {
        match *self {
            TbfHeader::TbfHeaderV1(_) => (0, 0),
            TbfHeader::TbfHeaderV2(hd) => hd.writeable_regions.map_or((0, 0), |wr| {
                if wr.len() > index {
                    (
                        wr[index].writeable_flash_region_offset,
                        wr[index].writeable_flash_region_size,
                    )
                } else {
                    (0, 0)
                }
            }),
            _ => (0, 0),
        }
    }
}

/// Converts a pointer to memory to a TbfHeader struct
///
/// This function takes a pointer to arbitrary memory and optionally returns a
/// TBF header struct. This function will validate the header checksum, but does
/// not perform sanity or security checking on the structure.
unsafe fn parse_and_validate_tbf_header(address: *const u8) -> Option<TbfHeader> {
    let version = *(address as *const u16);

    match version {
        1 => {
            let tbf_header = &*(address as *const TbfHeaderV1);

            let checksum = tbf_header.version ^ tbf_header.total_size ^ tbf_header.entry_offset
                ^ tbf_header.rel_data_offset ^ tbf_header.rel_data_size
                ^ tbf_header.text_offset ^ tbf_header.text_size
                ^ tbf_header.got_offset ^ tbf_header.got_size
                ^ tbf_header.data_offset ^ tbf_header.data_size
                ^ tbf_header.bss_mem_offset ^ tbf_header.bss_size
                ^ tbf_header.min_stack_len ^ tbf_header.min_app_heap_len
                ^ tbf_header.min_kernel_heap_len
                ^ tbf_header.pkg_name_offset ^ tbf_header.pkg_name_size;

            if checksum != tbf_header.checksum {
                None
            } else {
                Some(TbfHeader::TbfHeaderV1(tbf_header))
            }
        }

        2 => {
            let tbf_header_base = &*(address as *const TbfHeaderV2Base);

            // Some sanity checking. Make sure the header isn't longer than the
            // total app. Make sure the total app fits inside a reasonable size
            // of flash.
            if tbf_header_base.header_size as u32 >= tbf_header_base.total_size
                || tbf_header_base.total_size > 0x010000000
            {
                return None;
            }

            // Calculate checksum. The checksum is the XOR of each 4 byte word
            // in the header.
            let mut chunks = tbf_header_base.header_size as usize / 4;
            let mut leftover_bytes = 0;
            if chunks * 4 != tbf_header_base.header_size as usize {
                chunks += 1;
                leftover_bytes = tbf_header_base.header_size as usize - (chunks * 4);
            }
            let mut checksum: u32 = 0;
            let header = slice::from_raw_parts(address as *const u32, chunks);
            for (i, chunk) in header.iter().enumerate() {
                if i == 3 {
                    // Skip the checksum field.
                } else if i == chunks - 1 && leftover_bytes != 0 {
                    // In this case, we don't want to use the entire word.
                    checksum ^= *chunk & (0xFFFFFFFF >> (4 - leftover_bytes));
                } else {
                    checksum ^= *chunk;
                }
            }

            if checksum != tbf_header_base.checksum {
                return None;
            }

            // Skip the base of the header.
            let mut offset = mem::size_of::<TbfHeaderV2Base>() as isize;
            let mut remaining_length = tbf_header_base.header_size as usize - offset as usize;

            // Check if this is a real app or just padding. Padding apps are
            // identified by not having any options.
            if remaining_length == 0 {
                // Just padding.
                if checksum == tbf_header_base.checksum {
                    Some(TbfHeader::Padding(tbf_header_base))
                } else {
                    None
                }
            } else {
                // This is an actual app.

                // Places to save fields that we parse out of the header
                // options.
                let mut main_pointer: Option<&TbfHeaderV2Main> = None;
                let mut wfr_pointer: Option<
                    &'static [TbfHeaderV2WriteableFlashRegion],
                > = None;
                let mut app_name_str = "";

                // Loop through the header looking for known options.
                while remaining_length > mem::size_of::<TbfHeaderTlv>() {
                    let tbf_tlv_header = &*(address.offset(offset) as *const TbfHeaderTlv);

                    remaining_length -= mem::size_of::<TbfHeaderTlv>();
                    offset += mem::size_of::<TbfHeaderTlv>() as isize;

                    // Only parse known TLV blocks. There is no type 0.
                    if (tbf_tlv_header.tipe as u16) < TbfHeaderTypes::Unused as u16
                        && (tbf_tlv_header.tipe as u16) > 0
                    {
                        // This lets us skip unknown header types.

                        match tbf_tlv_header.tipe {
                            TbfHeaderTypes::TbfHeaderMain => /* Main */ {
                                if remaining_length >= mem::size_of::<TbfHeaderV2Main>() &&
                                   tbf_tlv_header.length as usize == mem::size_of::<TbfHeaderV2Main>() {
                                    let tbf_main = &*(address.offset(offset) as *const TbfHeaderV2Main);
                                    main_pointer = Some(tbf_main);
                                }
                            }
                            TbfHeaderTypes::TbfHeaderWriteableFlashRegions => /* Writeable Flash Regions */ {
                                // Length must be a multiple of the size of a region definition.
                                if tbf_tlv_header.length as usize % mem::size_of::<TbfHeaderV2WriteableFlashRegion>() == 0 {
                                    let number_regions = tbf_tlv_header.length as usize / mem::size_of::<TbfHeaderV2WriteableFlashRegion>();
                                    let region_start = &*(address.offset(offset) as *const TbfHeaderV2WriteableFlashRegion);
                                    let regions = slice::from_raw_parts(region_start, number_regions);
                                    wfr_pointer = Some(regions);
                                }
                            }
                            TbfHeaderTypes::TbfHeaderPackageName => /* Package Name */ {
                                if remaining_length >= tbf_tlv_header.length as usize {
                                    let package_name_byte_array =
                                        slice::from_raw_parts(address.offset(offset), tbf_tlv_header.length as usize);
                                    let _ = str::from_utf8(package_name_byte_array).map(|name_str| { app_name_str = name_str; });
                                }
                            }
                            TbfHeaderTypes::Unused => {}
                        }
                    }

                    // All TLV blocks are padded to 4 bytes, so we need to skip
                    // more if the length is not a multiple of 4.
                    remaining_length -= align4!(tbf_tlv_header.length) as usize;
                    offset += align4!(tbf_tlv_header.length) as isize;
                }

                let tbf_header = TbfHeaderV2 {
                    base: tbf_header_base,
                    main: main_pointer,
                    package_name: Some(app_name_str),
                    writeable_regions: wfr_pointer,
                };

                Some(TbfHeader::TbfHeaderV2(tbf_header))
            }
        }

        _ => None,
    }
}

#[derive(Default)]
struct StoredRegs {
    r4: usize,
    r5: usize,
    r6: usize,
    r7: usize,
    r8: usize,
    r9: usize,
    r10: usize,
    r11: usize,
}

/// State for helping with debugging apps.
///
/// These pointers and counters are not strictly required for kernel operation,
/// but provide helpful information when an app crashes.
struct ProcessDebug {
    /// Where the process has started its heap in RAM.
    app_heap_start_pointer: Option<*const u8>,

    /// Where the start of the stack is for the process. If the kernel does the
    /// PIC setup for this app then we know this, otherwise we need the app to
    /// tell us where it put its stack.
    app_stack_start_pointer: Option<*const u8>,

    /// How low have we ever seen the stack pointer.
    min_stack_pointer: *const u8,

    /// How many syscalls have occurred since the process started.
    syscall_count: Cell<usize>,

    /// What was the most recent syscall.
    last_syscall: Cell<Option<Syscall>>,

    /// How many callbacks were dropped because the queue was insufficiently
    /// long.
    dropped_callback_count: Cell<usize>,

    /// How many times this process has entered into a fault condition and the
    /// kernel has restarted it.
    restart_count: Cell<usize>,
}

pub struct Process<'a> {
    /// Application memory layout:
    ///
    /// ```text
    ///     ╒════════ ← memory[memory.len()]
    ///  ╔═ │ Grant
    ///     │   ↓
    ///  D  │ ──────  ← kernel_memory_break
    ///  Y  │
    ///  N  │ ──────  ← app_break
    ///  A  │
    ///  M  │   ↑
    ///     │  Heap
    ///  ╠═ │ ──────  ← app_heap_start
    ///     │  Data
    ///  F  │ ──────  ← data_start_pointer
    ///  I  │ Stack
    ///  X  │   ↓
    ///  E  │
    ///  D  │ ──────  ← current_stack_pointer
    ///     │
    ///  ╚═ ╘════════ ← memory[0]
    /// ```
    ///
    /// The process's memory.
    memory: &'static mut [u8],

    /// Pointer to the end of the allocated (and MPU protected) grant region.
    kernel_memory_break: *const u8,

    /// Copy of where the kernel memory break is when the app is first started.
    /// This is handy if the app is restarted so we know where to reset
    /// the kernel_memory break to without having to recalculate it.
    original_kernel_memory_break: *const u8,

    /// Pointer to the end of process RAM that has been sbrk'd to the process.
    app_break: *const u8,
    original_app_break: *const u8,

    /// Saved when the app switches to the kernel.
    current_stack_pointer: *const u8,
    original_stack_pointer: *const u8,

    /// Process text segment
    text: &'static [u8],

    /// Collection of pointers to the TBF header in flash.
    header: TbfHeader,

    /// Saved each time the app switches to the kernel.
    stored_regs: StoredRegs,

    /// The PC to jump to when switching back to the app.
    yield_pc: usize,

    /// Process State Register.
    psr: usize,

    /// Whether the scheduler can schedule this app.
    state: State,

    /// How to deal with Faults occurring in the process
    fault_response: FaultResponse,

    /// MPU regions are saved as a pointer-size pair.
    ///
    /// size is encoded as X where
    /// SIZE = 2<sup>(X + 1)</sup> and X >= 4.
    ///
    /// A null pointer represents an empty region.
    ///
    /// #### Invariants
    ///
    /// The pointer must be aligned to the size. E.g. if the size is 32 bytes, the pointer must be
    /// 32-byte aligned.
    mpu_regions: [Cell<(*const u8, math::PowerOfTwo)>; 5],

    /// Essentially a list of callbacks that want to call functions in the
    /// process.
    tasks: RingBuffer<'a, Task>,

    /// Name of the app. Public so that IPC can use it.
    pub package_name: &'static str,

    /// Values kept so that we can print useful debug messages when apps fault.
    debug: ProcessDebug,
}

// Stores the current number of callbacks enqueued + processes in Running state
static mut HAVE_WORK: VolatileCell<usize> = VolatileCell::new(0);

pub fn processes_blocked() -> bool {
    unsafe { HAVE_WORK.get() == 0 }
}

impl<'a> Process<'a> {
    pub fn schedule_ipc(&mut self, from: AppId, cb_type: IPCType) {
        unsafe {
            HAVE_WORK.set(HAVE_WORK.get() + 1);
        }
        let ret = self.tasks.enqueue(Task::IPC((from, cb_type)));

        // Make a note that we lost this callback if the enqueue function
        // fails.
        if ret == false {
            self.debug
                .dropped_callback_count
                .set(self.debug.dropped_callback_count.get() + 1);
        }
    }

    pub fn current_state(&self) -> State {
        self.state
    }

    pub fn yield_state(&mut self) {
        if self.state == State::Running {
            self.state = State::Yielded;
            unsafe {
                HAVE_WORK.set(HAVE_WORK.get() - 1);
            }
        }
    }

    pub unsafe fn fault_state(&mut self) {
        write_volatile(&mut APP_FAULT, 0);
        self.state = State::Fault;

        match self.fault_response {
            FaultResponse::Panic => {
                // process faulted. Panic and print status
                panic!("Process {} had a fault", self.package_name);
            }
            FaultResponse::Restart => {
                // Remove the tasks that were scheduled for the app from the
                // amount of work queue.
                if HAVE_WORK.get() < self.tasks.len() {
                    // This case should never happen.
                    HAVE_WORK.set(0);
                } else {
                    HAVE_WORK.set(HAVE_WORK.get() - self.tasks.len());
                }

                // And remove those tasks
                self.tasks.empty();

                // Mark that we restarted this process.
                self.debug
                    .restart_count
                    .set(self.debug.restart_count.get() + 1);

                // Reset some state for the process.
                self.debug.syscall_count.set(0);
                self.debug.last_syscall.set(None);
                self.debug.dropped_callback_count.set(0);

                // We are going to start this process over again, so need
                // the init_fn location.
                let app_flash_address = self.flash_start();
                let init_fn = app_flash_address
                    .offset(self.header.get_init_function_offset() as isize)
                    as usize;
                self.yield_pc = init_fn;
                self.psr = 0x01000000;
                self.state = State::Yielded;

                // Need to reset the grant region.
                self.grant_ptrs_reset();
                self.kernel_memory_break = self.original_kernel_memory_break;

                // Reset other memory pointers.
                self.app_break = self.original_app_break;
                self.current_stack_pointer = self.original_stack_pointer;

                // And queue up this app to be restarted.
                let flash_protected_size = self.header.get_protected_size() as usize;
                let flash_app_start = app_flash_address as usize + flash_protected_size;

                self.tasks.enqueue(Task::FunctionCall(FunctionCall {
                    pc: init_fn,
                    r0: flash_app_start,
                    r1: self.memory.as_ptr() as usize,
                    r2: self.memory.len() as usize,
                    r3: self.app_break as usize,
                }));

                HAVE_WORK.set(HAVE_WORK.get() + 1);
            }
        }
    }

    pub fn dequeue_task(&mut self) -> Option<Task> {
        self.tasks.dequeue().map(|cb| {
            unsafe {
                HAVE_WORK.set(HAVE_WORK.get() - 1);
            }
            cb
        })
    }

    pub fn mem_start(&self) -> *const u8 {
        self.memory.as_ptr()
    }

    pub fn mem_end(&self) -> *const u8 {
        unsafe { self.memory.as_ptr().offset(self.memory.len() as isize) }
    }

    pub fn flash_start(&self) -> *const u8 {
        self.text.as_ptr()
    }

    pub fn flash_non_protected_start(&self) -> *const u8 {
        ((self.text.as_ptr() as usize) + self.header.get_protected_size() as usize) as *const u8
    }

    pub fn flash_end(&self) -> *const u8 {
        unsafe { self.text.as_ptr().offset(self.text.len() as isize) }
    }

    pub fn kernel_memory_break(&self) -> *const u8 {
        self.kernel_memory_break
    }

    pub fn number_writeable_flash_regions(&self) -> usize {
        self.header.number_writeable_flash_regions()
    }

    pub fn get_writeable_flash_region(&self, region_index: usize) -> (u32, u32) {
        self.header.get_writeable_flash_region(region_index)
    }

    pub fn update_stack_start_pointer(&mut self, stack_pointer: *const u8) {
        if stack_pointer >= self.mem_start() && stack_pointer < self.mem_end() {
            self.debug.app_stack_start_pointer = Some(stack_pointer);

            // We also reset the minimum stack pointer because whatever value
            // we had could be entirely wrong by now.
            self.debug.min_stack_pointer = stack_pointer;
        }
    }

    pub fn update_heap_start_pointer(&mut self, heap_pointer: *const u8) {
        if heap_pointer >= self.mem_start() && heap_pointer < self.mem_end() {
            self.debug.app_heap_start_pointer = Some(heap_pointer);
        }
    }

    pub fn setup_mpu<MPU: mpu::MPU>(&self, mpu: &MPU) {
        // Text segment read/execute (no write)
        let text_start = self.text.as_ptr() as usize;
        let text_len = self.text.len();

        match MPU::create_region(
            0,
            text_start,
            text_len,
            mpu::ExecutePermission::ExecutionPermitted,
            mpu::AccessPermission::ReadOnly,
        ) {
            None => panic!(
                "Infeasible MPU allocation. Base {:#x}, Length: {:#x}",
                text_start, text_len
            ),
            Some(region) => mpu.set_mpu(region),
        }

        let data_start = self.memory.as_ptr() as usize;
        let data_len = self.memory.len();

        match MPU::create_region(
            1,
            data_start,
            data_len,
            mpu::ExecutePermission::ExecutionPermitted,
            mpu::AccessPermission::ReadWrite,
        ) {
            None => panic!(
                "Infeasible MPU allocation. Base {:#x}, Length: {:#x}",
                data_start, data_len
            ),
            Some(region) => mpu.set_mpu(region),
        }

        // Disallow access to grant region
        let grant_len = unsafe {
            math::PowerOfTwo::ceiling(
                self.memory.as_ptr().offset(self.memory.len() as isize) as u32
                    - (self.kernel_memory_break as u32),
            ).as_num::<u32>()
        };
        let grant_base = unsafe {
            self.memory
                .as_ptr()
                .offset(self.memory.len() as isize)
                .offset(-(grant_len as isize))
        };

        match MPU::create_region(
            2,
            grant_base as usize,
            grant_len as usize,
            mpu::ExecutePermission::ExecutionNotPermitted,
            mpu::AccessPermission::PrivilegedOnly,
        ) {
            None => panic!(
                "Infeasible MPU allocation. Base {:#x}, Length: {:#x}",
                grant_base as usize, grant_len
            ),
            Some(region) => mpu.set_mpu(region),
        }

        // Setup IPC MPU regions
        for (i, region) in self.mpu_regions.iter().enumerate() {
            if region.get().0 == ptr::null() {
                mpu.set_mpu(mpu::Region::empty(i + 3));
                continue;
            }
            match MPU::create_region(
                i + 3,
                region.get().0 as usize,
                region.get().1.as_num::<u32>() as usize,
                mpu::ExecutePermission::ExecutionPermitted,
                mpu::AccessPermission::ReadWrite,
            ) {
                None => panic!(
                    "Unexpected: Infeasible MPU allocation: Num: {}, \
                     Base: {:#x}, Length: {:#x}",
                    i + 3,
                    region.get().0 as usize,
                    region.get().1.as_num::<u32>()
                ),
                Some(region) => mpu.set_mpu(region),
            }
        }
    }

    pub fn add_mpu_region(&self, base: *const u8, size: u32) -> bool {
        if size >= 16 && size.count_ones() == 1 && (base as u32) % size == 0 {
            let mpu_size = math::PowerOfTwo::floor(size);
            for region in self.mpu_regions.iter() {
                if region.get().0 == ptr::null() {
                    region.set((base, mpu_size));
                    return true;
                } else if region.get().0 == base {
                    if region.get().1 < mpu_size {
                        region.set((base, mpu_size));
                    }
                    return true;
                }
            }
        }
        return false;
    }

    pub unsafe fn create(
        app_flash_address: *const u8,
        remaining_app_memory: *mut u8,
        remaining_app_memory_size: usize,
        fault_response: FaultResponse,
    ) -> (Option<&'static mut Process<'a>>, usize, usize) {
        if let Some(tbf_header) = parse_and_validate_tbf_header(app_flash_address) {
            let app_flash_size = tbf_header.get_total_size() as usize;

            // If this isn't an app (i.e. it is padding) or it is an app but it
            // isn't enabled, then we can skip it but increment past its flash.
            if !tbf_header.is_app() || !tbf_header.enabled() {
                return (None, app_flash_size, 0);
            }

            // Otherwise, actually load the app.
            let mut min_app_ram_size = tbf_header.get_minimum_app_ram_size();
            let package_name = tbf_header.get_package_name(app_flash_address);
            let init_fn =
                app_flash_address.offset(tbf_header.get_init_function_offset() as isize) as usize;

            // Set the initial process stack and memory to 128 bytes.
            let initial_stack_pointer = remaining_app_memory.offset(128);
            let initial_sbrk_pointer = remaining_app_memory.offset(128);

            // First determine how much space we need in the application's
            // memory space just for kernel and grant state. We need to make
            // sure we allocate enough memory just for that.

            // Make room for grant pointers.
            let grant_ptr_size = mem::size_of::<*const usize>();
            let grant_ptrs_num = read_volatile(&grant::CONTAINER_COUNTER);
            let grant_ptrs_offset = grant_ptrs_num * grant_ptr_size;

            // Allocate memory for callback ring buffer.
            let callback_size = mem::size_of::<Task>();
            let callback_len = 10;
            let callbacks_offset = callback_len * callback_size;

            // Make room to store this process's metadata.
            let process_struct_offset = mem::size_of::<Process>();

            // Need to make sure that the amount of memory we allocate for
            // this process at least covers this state.
            if min_app_ram_size
                < (grant_ptrs_offset + callbacks_offset + process_struct_offset) as u32
            {
                min_app_ram_size =
                    (grant_ptrs_offset + callbacks_offset + process_struct_offset) as u32;
            }

            // TODO round app_ram_size up to a closer MPU unit.
            // This is a very conservative approach that rounds up to power of
            // two. We should be able to make this closer to what we actually need.
            let app_ram_size = math::closest_power_of_two(min_app_ram_size) as usize;

            // Check that we can actually give this app this much memory.
            if app_ram_size > remaining_app_memory_size {
                panic!(
                    "{:?} failed to load. Insufficient memory. Requested {} have {}",
                    package_name, app_ram_size, remaining_app_memory_size
                );
            }

            let app_memory = slice::from_raw_parts_mut(remaining_app_memory, app_ram_size);

            // Set up initial grant region.
            let mut kernel_memory_break = app_memory.as_mut_ptr().offset(app_memory.len() as isize);

            // Now that we know we have the space we can setup the grant
            // pointers.
            kernel_memory_break = kernel_memory_break.offset(-(grant_ptrs_offset as isize));

            // Set all pointers to null.
            let opts =
                slice::from_raw_parts_mut(kernel_memory_break as *mut *const usize, grant_ptrs_num);
            for opt in opts.iter_mut() {
                *opt = ptr::null()
            }

            // Now that we know we have the space we can setup the memory
            // for the callbacks.
            kernel_memory_break = kernel_memory_break.offset(-(callbacks_offset as isize));

            // Set up ring buffer.
            let callback_buf =
                slice::from_raw_parts_mut(kernel_memory_break as *mut Task, callback_len);
            let tasks = RingBuffer::new(callback_buf);

            // Last thing is the process struct.
            kernel_memory_break = kernel_memory_break.offset(-(process_struct_offset as isize));
            let process_struct_memory_location = kernel_memory_break;

            // Determine the debug information to the best of our
            // understanding. If the app is doing all of the PIC fixup and
            // memory management we don't know much.
            let mut app_heap_start_pointer = None;
            let mut app_stack_start_pointer = None;

            // Create the Process struct in the app grant region.
            let mut process: &mut Process = mem::transmute(process_struct_memory_location);

            process.memory = app_memory;
            process.header = tbf_header;
            process.kernel_memory_break = kernel_memory_break;
            process.original_kernel_memory_break = kernel_memory_break;
            process.app_break = initial_sbrk_pointer;
            process.original_app_break = initial_sbrk_pointer;
            process.current_stack_pointer = initial_stack_pointer;
            process.original_stack_pointer = initial_stack_pointer;

            process.text = slice::from_raw_parts(app_flash_address, app_flash_size);

            process.stored_regs = Default::default();
            process.yield_pc = init_fn;
            // Set the Thumb bit and clear everything else
            process.psr = 0x01000000;

            process.state = State::Yielded;
            process.fault_response = fault_response;

            process.mpu_regions = [
                Cell::new((ptr::null(), math::PowerOfTwo::zero())),
                Cell::new((ptr::null(), math::PowerOfTwo::zero())),
                Cell::new((ptr::null(), math::PowerOfTwo::zero())),
                Cell::new((ptr::null(), math::PowerOfTwo::zero())),
                Cell::new((ptr::null(), math::PowerOfTwo::zero())),
            ];
            process.tasks = tasks;
            process.package_name = package_name;

            process.debug = ProcessDebug {
                app_heap_start_pointer: app_heap_start_pointer,
                app_stack_start_pointer: app_stack_start_pointer,
                min_stack_pointer: initial_stack_pointer,
                syscall_count: Cell::new(0),
                last_syscall: Cell::new(None),
                dropped_callback_count: Cell::new(0),
                restart_count: Cell::new(0),
            };

            if (init_fn & 0x1) != 1 {
                panic!(
                    "{:?} process image invalid. \
                     init_fn address must end in 1 to be Thumb, got {:#X}",
                    package_name, init_fn
                );
            }

            let flash_protected_size = process.header.get_protected_size() as usize;
            let flash_app_start = app_flash_address as usize + flash_protected_size;

            process.tasks.enqueue(Task::FunctionCall(FunctionCall {
                pc: init_fn,
                r0: flash_app_start,
                r1: process.memory.as_ptr() as usize,
                r2: process.memory.len() as usize,
                r3: process.app_break as usize,
            }));

            HAVE_WORK.set(HAVE_WORK.get() + 1);

            return (Some(process), app_flash_size, app_ram_size);
        }
        (None, 0, 0)
    }

    pub fn sbrk(&mut self, increment: isize) -> Result<*const u8, Error> {
        let new_break = unsafe { self.app_break.offset(increment) };
        self.brk(new_break)
    }

    pub fn brk(&mut self, new_break: *const u8) -> Result<*const u8, Error> {
        if new_break < self.mem_start() || new_break >= self.mem_end() {
            Err(Error::AddressOutOfBounds)
        } else if new_break > self.kernel_memory_break {
            Err(Error::OutOfMemory)
        } else {
            let old_break = self.app_break;
            self.app_break = new_break;
            Ok(old_break)
        }
    }

    pub fn in_exposed_bounds(&self, buf_start_addr: *const u8, size: usize) -> bool {
        let buf_end_addr = unsafe { buf_start_addr.offset(size as isize) };

        buf_start_addr >= self.mem_start() && buf_end_addr <= self.mem_end()
    }

    pub unsafe fn alloc(&mut self, size: usize) -> Option<&mut [u8]> {
        let new_break = self.kernel_memory_break.offset(-(size as isize));
        if new_break < self.app_break {
            None
        } else {
            self.kernel_memory_break = new_break;
            Some(slice::from_raw_parts_mut(new_break as *mut u8, size))
        }
    }

    pub unsafe fn free<T>(&mut self, _: *mut T) {}

    unsafe fn grant_ptr<T>(&self, grant_num: usize) -> *mut *mut T {
        let grant_num = grant_num as isize;
        (self.mem_end() as *mut *mut T).offset(-(grant_num + 1))
    }

    /// Reset all `grant_ptr`s to NULL.
    unsafe fn grant_ptrs_reset(&self) {
        let grant_ptrs_num = read_volatile(&grant::CONTAINER_COUNTER);
        for grant_num in 0..grant_ptrs_num {
            let grant_num = grant_num as isize;
            let ctr_ptr = (self.mem_end() as *mut *mut usize).offset(-(grant_num + 1));
            write_volatile(ctr_ptr, 0 as *mut usize);
        }
    }

    pub unsafe fn grant_for<T>(&mut self, grant_num: usize) -> *mut T {
        *self.grant_ptr(grant_num)
    }

    pub unsafe fn grant_for_or_alloc<T: Default>(&mut self, grant_num: usize) -> Option<*mut T> {
        let ctr_ptr = self.grant_ptr::<T>(grant_num);
        if (*ctr_ptr).is_null() {
            self.alloc(mem::size_of::<T>()).map(|root_arr| {
                let root_ptr = root_arr.as_mut_ptr() as *mut T;
                // Initialize the grant contents using ptr::write, to
                // ensure that we don't try to drop the contents of
                // uninitialized memory when T implements Drop.
                write(root_ptr, Default::default());
                // Record the location in the grant pointer.
                write_volatile(ctr_ptr, root_ptr);
                root_ptr
            })
        } else {
            Some(*ctr_ptr)
        }
    }

    pub fn pop_syscall_stack(&mut self) {
        let pspr = self.current_stack_pointer as *const usize;
        unsafe {
            self.yield_pc = read_volatile(pspr.offset(6));
            self.psr = read_volatile(pspr.offset(7));
            self.current_stack_pointer =
                (self.current_stack_pointer as *mut usize).offset(8) as *mut u8;
            if self.current_stack_pointer < self.debug.min_stack_pointer {
                self.debug.min_stack_pointer = self.current_stack_pointer;
            }
        }
    }

    /// Context switch to the process.
    pub unsafe fn push_function_call(&mut self, callback: FunctionCall) {
        HAVE_WORK.set(HAVE_WORK.get() + 1);

        self.state = State::Running;
        // Fill in initial stack expected by SVC handler
        // Top minus 8 u32s for r0-r3, r12, lr, pc and xPSR
        let stack_bottom = (self.current_stack_pointer as *mut usize).offset(-8);
        write_volatile(stack_bottom.offset(7), self.psr);
        write_volatile(stack_bottom.offset(6), callback.pc | 1);

        // Set the LR register to the saved PC so the callback returns to
        // wherever wait was called. Set lowest bit to one because of THUMB
        // instruction requirements.
        write_volatile(stack_bottom.offset(5), self.yield_pc | 0x1);
        write_volatile(stack_bottom, callback.r0);
        write_volatile(stack_bottom.offset(1), callback.r1);
        write_volatile(stack_bottom.offset(2), callback.r2);
        write_volatile(stack_bottom.offset(3), callback.r3);

        self.current_stack_pointer = stack_bottom as *mut u8;
        if self.current_stack_pointer < self.debug.min_stack_pointer {
            self.debug.min_stack_pointer = self.current_stack_pointer;
        }
    }

    pub unsafe fn app_fault(&self) -> bool {
        read_volatile(&APP_FAULT) != 0
    }

    pub unsafe fn syscall_fired(&self) -> bool {
        read_volatile(&SYSCALL_FIRED) != 0
    }

    /// Context switch to the process.
    pub unsafe fn switch_to(&mut self) {
        write_volatile(&mut SYSCALL_FIRED, 0);
        let psp = switch_to_user(
            self.current_stack_pointer,
            mem::transmute(&mut self.stored_regs),
        );
        self.current_stack_pointer = psp;
        if self.current_stack_pointer < self.debug.min_stack_pointer {
            self.debug.min_stack_pointer = self.current_stack_pointer;
        }
    }

    pub fn svc_number(&self) -> Option<Syscall> {
        let psp = self.current_stack_pointer as *const *const u16;
        unsafe {
            let pcptr = read_volatile((psp as *const *const u16).offset(6));
            let svc_instr = read_volatile(pcptr.offset(-1));
            let svc_num = (svc_instr & 0xff) as u8;
            match svc_num {
                0 => Some(Syscall::YIELD),
                1 => Some(Syscall::SUBSCRIBE),
                2 => Some(Syscall::COMMAND),
                3 => Some(Syscall::ALLOW),
                4 => Some(Syscall::MEMOP),
                _ => None,
            }
        }
    }

    pub fn incr_syscall_count(&self) {
        self.debug
            .syscall_count
            .set(self.debug.syscall_count.get() + 1);
        self.debug.last_syscall.set(self.svc_number());
    }

    pub fn sp(&self) -> usize {
        self.current_stack_pointer as usize
    }

    pub fn lr(&self) -> usize {
        let pspr = self.current_stack_pointer as *const usize;
        unsafe { read_volatile(pspr.offset(5)) }
    }

    pub fn pc(&self) -> usize {
        let pspr = self.current_stack_pointer as *const usize;
        unsafe { read_volatile(pspr.offset(6)) }
    }

    pub fn r0(&self) -> usize {
        let pspr = self.current_stack_pointer as *const usize;
        unsafe { read_volatile(pspr) }
    }

    pub fn set_return_code(&mut self, return_code: ReturnCode) {
        let r: isize = return_code.into();
        self.set_r0(r);
    }

    pub fn set_r0(&mut self, val: isize) {
        let pspr = self.current_stack_pointer as *mut isize;
        unsafe { write_volatile(pspr, val) }
    }

    pub fn r1(&self) -> usize {
        let pspr = self.current_stack_pointer as *const usize;
        unsafe { read_volatile(pspr.offset(1)) }
    }

    pub fn r2(&self) -> usize {
        let pspr = self.current_stack_pointer as *const usize;
        unsafe { read_volatile(pspr.offset(2)) }
    }

    pub fn r3(&self) -> usize {
        let pspr = self.current_stack_pointer as *const usize;
        unsafe { read_volatile(pspr.offset(3)) }
    }

    pub fn r12(&self) -> usize {
        let pspr = self.current_stack_pointer as *const usize;
        unsafe { read_volatile(pspr.offset(4)) }
    }

    pub fn xpsr(&self) -> usize {
        let pspr = self.current_stack_pointer as *const usize;
        unsafe { read_volatile(pspr.offset(7)) }
    }

    pub unsafe fn fault_str<W: Write>(&mut self, writer: &mut W) {
        let _ccr = SCB_REGISTERS[0];
        let cfsr = SCB_REGISTERS[1];
        let hfsr = SCB_REGISTERS[2];
        let mmfar = SCB_REGISTERS[3];
        let bfar = SCB_REGISTERS[4];

        let iaccviol = (cfsr & 0x01) == 0x01;
        let daccviol = (cfsr & 0x02) == 0x02;
        let munstkerr = (cfsr & 0x08) == 0x08;
        let mstkerr = (cfsr & 0x10) == 0x10;
        let mlsperr = (cfsr & 0x20) == 0x20;
        let mmfarvalid = (cfsr & 0x80) == 0x80;

        let ibuserr = ((cfsr >> 8) & 0x01) == 0x01;
        let preciserr = ((cfsr >> 8) & 0x02) == 0x02;
        let impreciserr = ((cfsr >> 8) & 0x04) == 0x04;
        let unstkerr = ((cfsr >> 8) & 0x08) == 0x08;
        let stkerr = ((cfsr >> 8) & 0x10) == 0x10;
        let lsperr = ((cfsr >> 8) & 0x20) == 0x20;
        let bfarvalid = ((cfsr >> 8) & 0x80) == 0x80;

        let undefinstr = ((cfsr >> 16) & 0x01) == 0x01;
        let invstate = ((cfsr >> 16) & 0x02) == 0x02;
        let invpc = ((cfsr >> 16) & 0x04) == 0x04;
        let nocp = ((cfsr >> 16) & 0x08) == 0x08;
        let unaligned = ((cfsr >> 16) & 0x100) == 0x100;
        let divbysero = ((cfsr >> 16) & 0x200) == 0x200;

        let vecttbl = (hfsr & 0x02) == 0x02;
        let forced = (hfsr & 0x40000000) == 0x40000000;

        let _ = writer.write_fmt(format_args!("\r\n---| Fault Status |---\r\n"));

        if iaccviol {
            let _ = writer.write_fmt(format_args!(
                "Instruction Access Violation:       {}\r\n",
                iaccviol
            ));
        }
        if daccviol {
            let _ = writer.write_fmt(format_args!(
                "Data Access Violation:              {}\r\n",
                daccviol
            ));
        }
        if munstkerr {
            let _ = writer.write_fmt(format_args!(
                "Memory Management Unstacking Fault: {}\r\n",
                munstkerr
            ));
        }
        if mstkerr {
            let _ = writer.write_fmt(format_args!(
                "Memory Management Stacking Fault:   {}\r\n",
                mstkerr
            ));
        }
        if mlsperr {
            let _ = writer.write_fmt(format_args!(
                "Memory Management Lazy FP Fault:    {}\r\n",
                mlsperr
            ));
        }

        if ibuserr {
            let _ = writer.write_fmt(format_args!(
                "Instruction Bus Error:              {}\r\n",
                ibuserr
            ));
        }
        if preciserr {
            let _ = writer.write_fmt(format_args!(
                "Precise Data Bus Error:             {}\r\n",
                preciserr
            ));
        }
        if impreciserr {
            let _ = writer.write_fmt(format_args!(
                "Imprecise Data Bus Error:           {}\r\n",
                impreciserr
            ));
        }
        if unstkerr {
            let _ = writer.write_fmt(format_args!(
                "Bus Unstacking Fault:               {}\r\n",
                unstkerr
            ));
        }
        if stkerr {
            let _ = writer.write_fmt(format_args!(
                "Bus Stacking Fault:                 {}\r\n",
                stkerr
            ));
        }
        if lsperr {
            let _ = writer.write_fmt(format_args!(
                "Bus Lazy FP Fault:                  {}\r\n",
                lsperr
            ));
        }

        if undefinstr {
            let _ = writer.write_fmt(format_args!(
                "Undefined Instruction Usage Fault:  {}\r\n",
                undefinstr
            ));
        }
        if invstate {
            let _ = writer.write_fmt(format_args!(
                "Invalid State Usage Fault:          {}\r\n",
                invstate
            ));
        }
        if invpc {
            let _ = writer.write_fmt(format_args!(
                "Invalid PC Load Usage Fault:        {}\r\n",
                invpc
            ));
        }
        if nocp {
            let _ = writer.write_fmt(format_args!(
                "No Coprocessor Usage Fault:         {}\r\n",
                nocp
            ));
        }
        if unaligned {
            let _ = writer.write_fmt(format_args!(
                "Unaligned Access Usage Fault:       {}\r\n",
                unaligned
            ));
        }
        if divbysero {
            let _ = writer.write_fmt(format_args!(
                "Divide By Zero:                     {}\r\n",
                divbysero
            ));
        }

        if vecttbl {
            let _ = writer.write_fmt(format_args!(
                "Bus Fault on Vector Table Read:     {}\r\n",
                vecttbl
            ));
        }
        if forced {
            let _ = writer.write_fmt(format_args!(
                "Forced Hard Fault:                  {}\r\n",
                forced
            ));
        }

        if mmfarvalid {
            let _ = writer.write_fmt(format_args!(
                "Faulting Memory Address:            {:#010X}\r\n",
                mmfar
            ));
        }
        if bfarvalid {
            let _ = writer.write_fmt(format_args!(
                "Bus Fault Address:                  {:#010X}\r\n",
                bfar
            ));
        }

        if cfsr == 0 && hfsr == 0 {
            let _ = writer.write_fmt(format_args!("No faults detected.\r\n"));
        } else {
            let _ = writer.write_fmt(format_args!(
                "Fault Status Register (CFSR):       {:#010X}\r\n",
                cfsr
            ));
            let _ = writer.write_fmt(format_args!(
                "Hard Fault Status Register (HFSR):  {:#010X}\r\n",
                hfsr
            ));
        }
    }

    pub unsafe fn statistics_str<W: Write>(&mut self, writer: &mut W) {
        // Flash
        let flash_end = self.text.as_ptr().offset(self.text.len() as isize) as usize;
        let flash_start = self.text.as_ptr() as usize;
        let flash_protected_size = self.header.get_protected_size() as usize;
        let flash_app_start = flash_start + flash_protected_size;
        let flash_app_size = flash_end - flash_app_start;
        let flash_init_fn = flash_start + self.header.get_init_function_offset() as usize;

        // SRAM addresses
        let sram_end = self.memory.as_ptr().offset(self.memory.len() as isize) as usize;
        let sram_grant_start = self.kernel_memory_break as usize;
        let sram_heap_end = self.app_break as usize;
        let sram_heap_start = self.debug.app_heap_start_pointer.unwrap_or(ptr::null()) as usize;
        let sram_stack_start = self.debug.app_stack_start_pointer.unwrap_or(ptr::null()) as usize;
        let sram_stack_bottom = self.debug.min_stack_pointer as usize;
        let sram_start = self.memory.as_ptr() as usize;

        // SRAM sizes
        let sram_grant_size = sram_end - sram_grant_start;
        let sram_heap_size = sram_heap_end - sram_heap_start;
        let sram_data_size = sram_heap_start - sram_stack_start;
        let sram_stack_size = sram_stack_start - sram_stack_bottom;
        let sram_grant_allocated = sram_end - sram_grant_start;
        let sram_heap_allocated = sram_grant_start - sram_heap_start;
        let sram_stack_allocated = sram_stack_start - sram_start;
        let sram_data_allocated = sram_data_size as usize;

        // checking on sram
        let mut sram_grant_error_str = "          ";
        if sram_grant_size > sram_grant_allocated {
            sram_grant_error_str = " EXCEEDED!"
        }
        let mut sram_heap_error_str = "          ";
        if sram_heap_size > sram_heap_allocated {
            sram_heap_error_str = " EXCEEDED!"
        }
        let mut sram_stack_error_str = "          ";
        if sram_stack_size > sram_stack_allocated {
            sram_stack_error_str = " EXCEEDED!"
        }

        // application statistics
        let events_queued = self.tasks.len();
        let syscall_count = self.debug.syscall_count.get();
        let last_syscall = self.debug.last_syscall.get();
        let dropped_callback_count = self.debug.dropped_callback_count.get();
        let restart_count = self.debug.restart_count.get();

        // register values
        let (r0, r1, r2, r3, r12, sp, lr, pc, xpsr) = (
            self.r0(),
            self.r1(),
            self.r2(),
            self.r3(),
            self.r12(),
            self.sp(),
            self.lr(),
            self.pc(),
            self.xpsr(),
        );

        let _ = writer.write_fmt(format_args!(
            "\
             App: {}   -   [{:?}]\
             \r\n Events Queued: {}   Syscall Count: {}   Dropped Callback Count: {}\
             \n Restart Count: {}\n",
            self.package_name,
            self.state,
            events_queued,
            syscall_count,
            dropped_callback_count,
            restart_count,
        ));

        let _ = match last_syscall {
            Some(syscall) => writer.write_fmt(format_args!(" Last Syscall: {:?}", syscall)),
            None => writer.write_fmt(format_args!(" Last Syscall: None")),
        };

        let _ = writer.write_fmt(format_args!("\
\r\n\
\r\n ╔═══════════╤══════════════════════════════════════════╗\
\r\n ║  Address  │ Region Name    Used | Allocated (bytes)  ║\
\r\n ╚{:#010X}═╪══════════════════════════════════════════╝\
\r\n             │ ▼ Grant      {:6} | {:6}{}\
  \r\n  {:#010X} ┼───────────────────────────────────────────\
\r\n             │ Unused\
  \r\n  {:#010X} ┼───────────────────────────────────────────\
\r\n             │ ▲ Heap       {:6} | {:6}{}     S\
  \r\n  {:#010X} ┼─────────────────────────────────────────── R\
\r\n             │ Data         {:6} | {:6}               A\
  \r\n  {:#010X} ┼─────────────────────────────────────────── M\
\r\n             │ ▼ Stack      {:6} | {:6}{}\
  \r\n  {:#010X} ┼───────────────────────────────────────────\
\r\n             │ Unused\
  \r\n  {:#010X} ┴───────────────────────────────────────────\
\r\n             .....\
  \r\n  {:#010X} ┬─────────────────────────────────────────── F\
\r\n             │ App Flash    {:6}                        L\
  \r\n  {:#010X} ┼─────────────────────────────────────────── A\
\r\n             │ Protected    {:6}                        S\
  \r\n  {:#010X} ┴─────────────────────────────────────────── H\
\r\n\
  \r\n  R0 : {:#010X}    R6 : {:#010X}\
  \r\n  R1 : {:#010X}    R7 : {:#010X}\
  \r\n  R2 : {:#010X}    R8 : {:#010X}\
  \r\n  R3 : {:#010X}    R10: {:#010X}\
  \r\n  R4 : {:#010X}    R11: {:#010X}\
  \r\n  R5 : {:#010X}    R12: {:#010X}\
  \r\n  R9 : {:#010X} (Static Base Register)\
  \r\n  SP : {:#010X} (Process Stack Pointer)\
  \r\n  LR : {:#010X}\
  \r\n  PC : {:#010X}\
  \r\n YPC : {:#010X}\
\r\n",
  sram_end,
  sram_grant_size, sram_grant_allocated, sram_grant_error_str,
  sram_grant_start,
  sram_heap_end,
  sram_heap_size, sram_heap_allocated, sram_heap_error_str,
  sram_heap_start,
  sram_data_size, sram_data_allocated,
  sram_stack_start,
  sram_stack_size, sram_stack_allocated, sram_stack_error_str,
  sram_stack_bottom,
  sram_start,
  flash_end,
  flash_app_size,
  flash_app_start,
  flash_protected_size,
  flash_start,
  r0, self.stored_regs.r6,
  r1, self.stored_regs.r7,
  r2, self.stored_regs.r8,
  r3, self.stored_regs.r10,
  self.stored_regs.r4, self.stored_regs.r11,
  self.stored_regs.r5, r12,
  self.stored_regs.r9,
  sp,
  lr,
  pc,
  self.yield_pc,
  ));
        let _ = writer.write_fmt(format_args!(
            "\
             \r\n APSR: N {} Z {} C {} V {} Q {}\
             \r\n       GE {} {} {} {}",
            (xpsr >> 31) & 0x1,
            (xpsr >> 30) & 0x1,
            (xpsr >> 29) & 0x1,
            (xpsr >> 28) & 0x1,
            (xpsr >> 27) & 0x1,
            (xpsr >> 19) & 0x1,
            (xpsr >> 18) & 0x1,
            (xpsr >> 17) & 0x1,
            (xpsr >> 16) & 0x1,
        ));
        let ici_it = (((xpsr >> 25) & 0x3) << 6) | ((xpsr >> 10) & 0x3f);
        let thumb_bit = ((xpsr >> 24) & 0x1) == 1;
        let _ = writer.write_fmt(format_args!(
            "\
             \r\n EPSR: ICI.IT {:#04x}\
             \r\n       ThumbBit {} {}",
            ici_it,
            thumb_bit,
            if thumb_bit {
                ""
            } else {
                "!!ERROR - Cortex M Thumb only!"
            },
        ));
        let _ = writer.write_fmt(format_args!("\r\n To debug, run "));
        let _ = writer.write_fmt(format_args!(
            "`make debug RAM_START={:#x} FLASH_INIT={:#x}`",
            sram_start, flash_init_fn
        ));
        let _ = writer.write_fmt(format_args!(
            "\r\n in the app's folder and open the .lst file.\r\n\r\n"
        ));
    }
}
