use super::{CommonThreadInfo, NT_Elf, Pid};
#[cfg(target_arch = "x86")]
use crate::minidump_cpu::imp::MD_FLOATINGSAVEAREA_X86_REGISTERAREA_SIZE;
use crate::minidump_cpu::RawContextCPU;
use crate::Result;
use core::mem::size_of_val;
use libc;
use libc::user;
use memoffset;
use nix::sys::ptrace;
use nix::unistd;
use std::convert::TryInto;

const NUM_DEBUG_REGISTERS: usize = 8;

#[derive(Debug)]
pub struct ThreadInfoX86 {
    pub stack_pointer: usize,
    pub tgid: Pid, // thread group id
    pub ppid: Pid, // parent process
    pub regs: libc::user_regs_struct,
    pub fpregs: libc::user_fpregs_struct,
    #[cfg(target_arch = "x86_64")]
    pub dregs: [libc::c_ulonglong; NUM_DEBUG_REGISTERS],
    #[cfg(target_arch = "x86")]
    pub dregs: [libc::c_int; NUM_DEBUG_REGISTERS],
    #[cfg(target_arch = "x86")]
    pub fpxregs: libc::user_fpxregs_struct,
}

fn to_u128(slice: &[u32]) -> u128 {
    u128::from_ne_bytes(
        slice
            .iter()
            .map(|x| x.to_ne_bytes().to_vec())
            .flatten()
            .collect::<Vec<_>>()
            .as_slice()
            .try_into() // Make slice into fixed size array
            .unwrap(), // Which has to work as we know the numbers work out
    )
}

impl CommonThreadInfo for ThreadInfoX86 {}

impl ThreadInfoX86 {
    // nix currently doesn't support PTRACE_GETREGSET, so we have to do it ourselves
    fn getregset(pid: Pid) -> Result<libc::user_regs_struct> {
        Self::ptrace_get_data_via_io::<libc::user_regs_struct>(
            ptrace::Request::PTRACE_GETREGSET,
            Some(NT_Elf::NT_PRSTATUS),
            nix::unistd::Pid::from_raw(pid),
        )
    }

    // nix currently doesn't support PTRACE_GETREGSET, so we have to do it ourselves
    fn getfpregset(pid: Pid) -> Result<libc::user_fpregs_struct> {
        Self::ptrace_get_data_via_io::<libc::user_fpregs_struct>(
            ptrace::Request::PTRACE_GETREGSET,
            Some(NT_Elf::NT_PRFPREG),
            nix::unistd::Pid::from_raw(pid),
        )
    }

    // nix currently doesn't support PTRACE_GETFPREGS, so we have to do it ourselves
    fn getfpregs(pid: Pid) -> Result<libc::user_fpregs_struct> {
        Self::ptrace_get_data::<libc::user_fpregs_struct>(
            ptrace::Request::PTRACE_GETFPREGS,
            None,
            nix::unistd::Pid::from_raw(pid),
        )
    }

    // nix currently doesn't support PTRACE_GETFPXREGS, so we have to do it ourselves
    #[cfg(target_arch = "x86")]
    fn getfpxregs(pid: Pid) -> Result<libc::user_fpxregs_struct> {
        Self::ptrace_get_data::<libc::user_fpxregs_struct>(
            ptrace::Request::PTRACE_GETFPXREGS,
            None,
            nix::unistd::Pid::from_raw(pid),
        )
    }

    fn peek_user(pid: Pid, addr: ptrace::AddressType) -> nix::Result<libc::c_long> {
        Self::ptrace_peek(
            ptrace::Request::PTRACE_PEEKUSER,
            nix::unistd::Pid::from_raw(pid),
            addr,
            std::ptr::null_mut(),
        )
    }

    pub fn create_impl(_pid: Pid, tid: Pid) -> Result<Self> {
        let (ppid, tgid) = Self::get_ppid_and_tgid(tid)?;
        let regs = Self::getregset(tid).or_else(|_| ptrace::getregs(unistd::Pid::from_raw(tid)))?;
        let fpregs = Self::getfpregset(tid).or_else(|_| Self::getfpregs(tid))?;
        #[cfg(target_arch = "x86")]
        let fpxregs: libc::user_fpxregs_struct;
        #[cfg(target_arch = "x86")]
        {
            if cfg!(target_feature = "fxsr") {
                fpxregs = Self::getfpxregs(tid)?;
            } else {
                fpxregs = unsafe { mem::zeroed() };
            }
        }

        #[cfg(target_arch = "x86_64")]
        let mut dregs: [libc::c_ulonglong; NUM_DEBUG_REGISTERS] = [0; NUM_DEBUG_REGISTERS];
        #[cfg(target_arch = "x86")]
        let mut dregs: [libc::c_int; NUM_DEBUG_REGISTERS] = [0; NUM_DEBUG_REGISTERS];

        let debug_offset = memoffset::offset_of!(user, u_debugreg);
        let elem_offset = size_of_val(&dregs[0]);
        for idx in 0..NUM_DEBUG_REGISTERS {
            let chunk = Self::peek_user(
                tid,
                (debug_offset + idx * elem_offset) as ptrace::AddressType,
            )?;
            dregs[idx] = chunk as u64; // libc / ptrace is very messy wrt int types used...
        }

        #[cfg(target_arch = "x86_64")]
        let stack_pointer = regs.rsp as usize;
        #[cfg(target_arch = "x86")]
        let stack_pointer = regs.esp as usize;

        Ok(ThreadInfoX86 {
            stack_pointer,
            tgid,
            ppid,
            regs,
            fpregs,
            dregs,
            #[cfg(target_arch = "x86")]
            fpxregs,
        })
    }

    #[cfg(target_arch = "x86_64")]
    pub fn get_instruction_pointer(&self) -> libc::c_ulonglong {
        self.regs.rip
    }

    #[cfg(target_arch = "x86")]
    pub fn get_instruction_pointer(&self) -> libc::c_ulonglong {
        self.regs.eip
    }

    #[cfg(target_arch = "x86_64")]
    pub fn fill_cpu_context(&self, out: &mut RawContextCPU) {
        // out.context_flags = self.MD_CONTEXT_AMD64_FULL |
        //                      MD_CONTEXT_AMD64_SEGMENTS;

        out.cs = self.regs.cs as u16; // TODO: This is u64, do we loose information by doing this?

        out.ds = self.regs.ds as u16; // TODO: This is u64, do we loose information by doing this?
        out.es = self.regs.es as u16; // TODO: This is u64, do we loose information by doing this?
        out.fs = self.regs.fs as u16; // TODO: This is u64, do we loose information by doing this?
        out.gs = self.regs.gs as u16; // TODO: This is u64, do we loose information by doing this?

        out.ss = self.regs.ss as u16; // TODO: This is u64, do we loose information by doing this?
        out.eflags = self.regs.eflags as u32; // TODO: This is u64, do we loose information by doing this?

        out.dr0 = self.dregs[0];
        out.dr1 = self.dregs[1];
        out.dr2 = self.dregs[2];
        out.dr3 = self.dregs[3];
        // 4 and 5 deliberatly omitted because they aren't included in the minidump
        // format.
        out.dr6 = self.dregs[6];
        out.dr7 = self.dregs[7];

        out.rax = self.regs.rax;
        out.rcx = self.regs.rcx;
        out.rdx = self.regs.rdx;
        out.rbx = self.regs.rbx;

        out.rsp = self.regs.rsp;

        out.rbp = self.regs.rbp;
        out.rsi = self.regs.rsi;
        out.rdi = self.regs.rdi;
        out.r8 = self.regs.r8;
        out.r9 = self.regs.r9;
        out.r10 = self.regs.r10;
        out.r11 = self.regs.r11;
        out.r12 = self.regs.r12;
        out.r13 = self.regs.r13;
        out.r14 = self.regs.r14;
        out.r15 = self.regs.r15;

        out.rip = self.regs.rip;

        out.flt_save.control_word = self.fpregs.cwd;
        out.flt_save.status_word = self.fpregs.swd;
        out.flt_save.tag_word = self.fpregs.ftw as u8; // TODO: This is u16, do we loose information by doing this?
        out.flt_save.error_opcode = self.fpregs.fop;
        out.flt_save.error_offset = self.fpregs.rip as u32; // TODO: This is u64, do we loose information by doing this?
        out.flt_save.error_selector = 0; // We don't have this.
        out.flt_save.data_offset = self.fpregs.rdp as u32; // TODO: This is u64, do we loose information by doing this?
        out.flt_save.data_selector = 0; // We don't have this.
        out.flt_save.mx_csr = self.fpregs.mxcsr;
        out.flt_save.mx_csr_mask = self.fpregs.mxcr_mask;

        u128::from_ne_bytes(
            self.fpregs.st_space[0..4]
                .iter()
                .map(|x| x.to_ne_bytes().to_vec())
                .flatten()
                .collect::<Vec<_>>()
                .as_slice()
                .try_into()
                .unwrap(),
        );
        out.flt_save.float_registers[0] = to_u128(&self.fpregs.st_space[0..4]);
        out.flt_save.xmm_registers[0] = to_u128(&self.fpregs.xmm_space[0..4]);
        out.flt_save.xmm_registers[1] = to_u128(&self.fpregs.xmm_space[4..8]);
        // my_memcpy(&out.flt_save.float_registers, &self.fpregs.st_space, 8 * 16);
        // my_memcpy(&out.flt_save.xmm_registers, &self.fpregs.xmm_space, 16 * 16);
    }

    #[cfg(target_arch = "x86")]
    pub fn fill_cpu_context(&self, out: &mut RawContextCPU) {
        // out->context_flags = MD_CONTEXT_X86_ALL;

        out.dr0 = self.dregs[0];
        out.dr1 = self.dregs[1];
        out.dr2 = self.dregs[2];
        out.dr3 = self.dregs[3];
        // 4 and 5 deliberatly omitted because they aren't included in the minidump
        // format.
        out.dr6 = self.dregs[6];
        out.dr7 = self.dregs[7];

        out.gs = self.regs.xgs;
        out.fs = self.regs.xfs;
        out.es = self.regs.xes;
        out.ds = self.regs.xds;

        out.edi = self.regs.edi;
        out.esi = self.regs.esi;
        out.ebx = self.regs.ebx;
        out.edx = self.regs.edx;
        out.ecx = self.regs.ecx;
        out.eax = self.regs.eax;

        out.ebp = self.regs.ebp;
        out.eip = self.regs.eip;
        out.cs = self.regs.xcs;
        out.eflags = self.regs.eflags;
        out.esp = self.regs.esp;
        out.ss = self.regs.xss;

        out.float_save.control_word = self.fpregs.cwd;
        out.float_save.status_word = self.fpregs.swd;
        out.float_save.tag_word = self.fpregs.twd;
        out.float_save.error_offset = self.fpregs.fip;
        out.float_save.error_selector = self.fpregs.fcs;
        out.float_save.data_offset = self.fpregs.foo;
        out.float_save.data_selector = self.fpregs.fos;

        // 8 registers * 10 bytes per register.
        // my_memcpy(out->float_save.register_area, fpregs.st_space, 10 * 8);
        out.float_save.register_area = self
            .fpregs
            .st_space
            .iter()
            .map(|x| x.to_ne_bytes().to_vec())
            .flatten()
            .take(MD_FLOATINGSAVEAREA_X86_REGISTERAREA_SIZE)
            .collect::<Vec<_>>()
            .as_slice()
            .try_into() // Make slice into fixed size array
            .unwrap(); // Which has to work as we know the numbers work out

        // This matches the Intel fpsave format.
        let values = (
            self.fpregs.cwd as u16,
            self.fpregs.swd as u16,
            self.fpregs.twd as u16,
            self.fpxregs.fop as u16,
            self.fpxregs.fip as u32,
            self.fpxregs.fcs as u16,
            self.fpregs.foo as u32,
            self.fpregs.fos as u16,
            self.fpxregs.mxcsr as u32,
        );

        let mut idx = 0;
        for val in values {
            for byte in val.to_ne_bytes() {
                out.extended_registers[idx] = byte;
                idx += 1;
            }
        }

        // my_memcpy(out->extended_registers + 32, &fpxregs.st_space, 128);
        idx = 32;
        for val in self.fpxregs.st_space {
            for byte in val.to_ne_bytes() {
                out.extended_registers[idx] = byte;
                idx += 1;
            }
        }

        // my_memcpy(out->extended_registers + 160, &fpxregs.xmm_space, 128);
        idx = 160;
        for val in self.fpxregs.xmm_space {
            for byte in val.to_ne_bytes() {
                out.extended_registers[idx] = byte;
                idx += 1;
            }
        }
    }
}
