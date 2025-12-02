//! Process management syscalls
use alloc::sync::Arc;

use crate::{
    loader::get_app_data_by_name,
    mm::{translated_refmut, translated_str, VirtAddr},
    task::{
        add_task, current_task, current_user_token, exit_current_and_run_next,
        suspend_current_and_run_next,
    },
};
use crate::timer::get_time_us;

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(exit_code: i32) -> ! {
    trace!("kernel:pid[{}] sys_exit", current_task().unwrap().pid.0);
    exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel:pid[{}] sys_yield", current_task().unwrap().pid.0);
    suspend_current_and_run_next();
    0
}

pub fn sys_getpid() -> isize {
    trace!("kernel: sys_getpid pid:{}", current_task().unwrap().pid.0);
    current_task().unwrap().pid.0 as isize
}

pub fn sys_fork() -> isize {
    trace!("kernel:pid[{}] sys_fork", current_task().unwrap().pid.0);
    let current_task = current_task().unwrap();
    let new_task = current_task.fork();
    let new_pid = new_task.pid.0;
    // modify trap context of new_task, because it returns immediately after switching
    let trap_cx = new_task.inner_exclusive_access().get_trap_cx();
    // we do not have to move to next instruction since we have done it before
    // for child process, fork returns 0
    trap_cx.x[10] = 0;
    // add new task to scheduler
    add_task(new_task);
    new_pid as isize
}

pub fn sys_exec(path: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_exec", current_task().unwrap().pid.0);
    let token = current_user_token();
    let path = translated_str(token, path);
    if let Some(data) = get_app_data_by_name(path.as_str()) {
        let task = current_task().unwrap();
        task.exec(data);
        0
    } else {
        -1
    }
}

/// If there is not a child process whose pid is same as given, return -1.
/// Else if there is a child process but it is still running, return -2.
pub fn sys_waitpid(pid: isize, exit_code_ptr: *mut i32) -> isize {
    trace!("kernel::pid[{}] sys_waitpid [{}]", current_task().unwrap().pid.0, pid);
    let task = current_task().unwrap();
    // find a child process

    // ---- access current PCB exclusively
    let mut inner = task.inner_exclusive_access();
    if !inner
        .children
        .iter()
        .any(|p| pid == -1 || pid as usize == p.getpid())
    {
        return -1;
        // ---- release current PCB
    }
    let pair = inner.children.iter().enumerate().find(|(_, p)| {
        // ++++ temporarily access child PCB exclusively
        p.inner_exclusive_access().is_zombie() && (pid == -1 || pid as usize == p.getpid())
        // ++++ release child PCB
    });
    if let Some((idx, _)) = pair {
        let child = inner.children.remove(idx);
        // confirm that child will be deallocated after being removed from children list
        assert_eq!(Arc::strong_count(&child), 1);
        let found_pid = child.getpid();
        // ++++ temporarily access child PCB exclusively
        let exit_code = child.inner_exclusive_access().exit_code;
        // ++++ release child PCB
        *translated_refmut(inner.memory_set.token(), exit_code_ptr) = exit_code;
        found_pid as isize
    } else {
        -2
    }
    // ---- release current PCB automatically
}

/// YOUR JOB: get time with second and microsecond
/// HINT: You might reimplement it with virtual memory management.
/// HINT: What if [`TimeVal`] is splitted by two pages ?
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!(
        "kernel:pid[{}] sys_get_time",
        current_task().unwrap().pid.0
    );

    if ts.is_null() {
        return -1;
    }

    let us = get_time_us();
    let token = current_user_token();

    let sec = us / 1_000_000;
    let usec = us % 1_000_000;

    // Handle TimeVal fields separately to safely handle cross-page scenarios
    // First field: sec (offset 0)
    let sec_ptr = ts as *mut usize;
    let sec_ref = translated_refmut(token, sec_ptr);
    *sec_ref = sec;

    // Second field: usec (offset sizeof(usize))
    let usec_ptr = unsafe { (ts as *mut usize).add(1) };
    let usec_ref = translated_refmut(token, usec_ptr);
    *usec_ref = usec;

    0
}

/// YOUR JOB: Implement mmap.
pub fn sys_mmap(start: usize, len: usize, prot: usize) -> isize {
    trace!("kernel:pid[{}] sys_mmap start={:#x}, len={}, prot={:#x}",
           current_task().unwrap().pid.0, start, len, prot
    );

    // Check if start is page aligned
    if !VirtAddr::from(start).aligned() {
        return -1;
    }

    // Check prot validity: bits 0-2 only, and at least one permission bit set
    if prot & !0x7 != 0 || (prot & 0x7) == 0 {
        return -1;
    }

    // Zero length mapping is allowed but does nothing
    if len == 0 {
        return 0;
    }

    let task = current_task().unwrap();

    // Convert [start, start + len) to page-aligned range
    let start_va = VirtAddr::from(start);
    let end_va = VirtAddr::from(start + len);
    let start_vpn = start_va.floor();
    let end_vpn = end_va.ceil();

    // immutable operation
    {
        let inner = task.inner_exclusive_access();
        if inner.memory_set.is_range_mapped(start_vpn, end_vpn) {
            return -1; // Range already has mappings
        }
    }

    // mutable operation
    {
        let mut inner = task.inner_exclusive_access();
        // Convert prot to MapPermission
        let map_perm = crate::mm::MapPermission::from_prot(prot);
        // Insert the framed area
        inner.memory_set.insert_framed_area(start_va, end_va, map_perm);
    }

    0
}

/// YOUR JOB: Implement munmap.
pub fn sys_munmap(start: usize, len: usize) -> isize {
    trace!("kernel:pid[{}] sys_munmap start={:#x}, len={}",
           current_task().unwrap().pid.0, start, len
    );

    // Check if start is page aligned
    if !VirtAddr::from(start).aligned() {
        return -1;
    }

    // Zero length unmap is allowed but does nothing
    if len == 0 {
        return 0;
    }

    let task = current_task().unwrap();

    // Convert [start, start + len) to page-aligned range
    let start_va = VirtAddr::from(start);
    let end_va = VirtAddr::from(start + len);
    let start_vpn = start_va.floor();
    let end_vpn = end_va.ceil();

    // immutable operation
    {
        let inner = task.inner_exclusive_access();
        if !inner.memory_set.is_range_fully_mapped(start_vpn, end_vpn) {
            return -1; // Some pages in range are not mapped
        }
    }

    // mutable operation
    {
        let mut inner = task.inner_exclusive_access();
        inner.memory_set.remove_areas_in_range(start_vpn, end_vpn);
    }

    0
}

/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel:pid[{}] sys_sbrk", current_task().unwrap().pid.0);
    if let Some(old_brk) = current_task().unwrap().change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}

/// YOUR JOB: Implement spawn.
/// HINT: fork + exec =/= spawn
pub fn sys_spawn(path: *const u8) -> isize {
    trace!(
        "kernel:pid[{}] sys_spawn",
        current_task().unwrap().pid.0
    );
    let token = current_user_token();
    let path = translated_str(token, path);
    if let Some(data) = get_app_data_by_name(path.as_str()) {
        let current_task = current_task().unwrap();
        let new_task = current_task.spawn(data);
        let new_pid = new_task.pid.0;
        add_task(new_task.clone());
        new_pid as isize
    } else {
        -1
    }
}

// YOUR JOB: Set task priority.
pub fn sys_set_priority(prio: isize) -> isize {
    trace!(
        "kernel:pid[{}] sys_set_priority prio={}",
        current_task().unwrap().pid.0, prio
    );
    -1
}
