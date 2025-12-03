//! File and filesystem-related syscalls
use crate::fs::{open_file, linkat, unlinkat, OpenFlags, Stat, StatMode};
use crate::mm::{translated_byte_buffer, translated_refmut, translated_str, UserBuffer};
use crate::task::{current_task, current_user_token};

pub fn sys_write(fd: usize, buf: *const u8, len: usize) -> isize {
    trace!("kernel:pid[{}] sys_write", current_task().unwrap().pid.0);
    let token = current_user_token();
    let task = current_task().unwrap();
    let inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if let Some(file) = &inner.fd_table[fd] {
        if !file.writable() {
            return -1;
        }
        let file = file.clone();
        // release current task TCB manually to avoid multi-borrow
        drop(inner);
        file.write(UserBuffer::new(translated_byte_buffer(token, buf, len))) as isize
    } else {
        -1
    }
}

pub fn sys_read(fd: usize, buf: *const u8, len: usize) -> isize {
    trace!("kernel:pid[{}] sys_read", current_task().unwrap().pid.0);
    let token = current_user_token();
    let task = current_task().unwrap();
    let inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if let Some(file) = &inner.fd_table[fd] {
        let file = file.clone();
        if !file.readable() {
            return -1;
        }
        // release current task TCB manually to avoid multi-borrow
        drop(inner);
        trace!("kernel: sys_read .. file.read");
        file.read(UserBuffer::new(translated_byte_buffer(token, buf, len))) as isize
    } else {
        -1
    }
}

pub fn sys_open(path: *const u8, flags: u32) -> isize {
    trace!("kernel:pid[{}] sys_open", current_task().unwrap().pid.0);
    let task = current_task().unwrap();
    let token = current_user_token();
    let path = translated_str(token, path);
    if let Some(inode) = open_file(path.as_str(), OpenFlags::from_bits(flags).unwrap()) {
        let mut inner = task.inner_exclusive_access();
        let fd = inner.alloc_fd();
        inner.fd_table[fd] = Some(inode);
        fd as isize
    } else {
        -1
    }
}

pub fn sys_close(fd: usize) -> isize {
    trace!("kernel:pid[{}] sys_close", current_task().unwrap().pid.0);
    let task = current_task().unwrap();
    let mut inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if inner.fd_table[fd].is_none() {
        return -1;
    }
    inner.fd_table[fd].take();
    0
}

/// YOUR JOB: Implement fstat.
pub fn sys_fstat(fd: usize, st: *mut Stat) -> isize {
    trace!(
        "kernel:pid[{}] sys_fstat fd={}",
        current_task().unwrap().pid.0, fd
    );

    let task = current_task().unwrap();
    let file = {
        let inner = task.inner_exclusive_access();
        if fd >= inner.fd_table.len() {
            return -1;
        }
        match inner.fd_table[fd].as_ref() {
            Some(file) => file.clone(),
            None => return -1,
        }
    }; // Release the borrow here

    let mut stat = Stat::empty();
    file.stat(&mut stat);

    // Copy Stat structure to user space
    let dev_ptr = st as *mut u64;
    let ino_ptr = unsafe { dev_ptr.add(1) };
    let mode_ptr = unsafe { ino_ptr.add(1) as *mut StatMode };
    let nlink_ptr = unsafe { mode_ptr.add(1) as *mut u32 };

    let token = current_user_token();
    let dev_ref = translated_refmut(token, dev_ptr);
    let ino_ref = translated_refmut(token, ino_ptr);
    let mode_ref = translated_refmut(token, mode_ptr);
    let nlink_ref = translated_refmut(token, nlink_ptr);

    *dev_ref = stat.dev;
    *ino_ref = stat.ino;
    *mode_ref = stat.mode;
    *nlink_ref = stat.nlink;

    0
}

/// YOUR JOB: Implement linkat.
pub fn sys_linkat(old_name: *const u8, new_name: *const u8) -> isize {
    trace!(
        "kernel:pid[{}] sys_linkat",
        current_task().unwrap().pid.0
    );

    let token = current_user_token();
    let new_name = translated_str(token, new_name);
    let old_name = translated_str(token, old_name);

    // Check if names are the same - this is an error condition
    if new_name == old_name {
        return -1;
    }

    if linkat(&old_name, &new_name) { 0 } else { -1 }
}

/// YOUR JOB: Implement unlinkat.
pub fn sys_unlinkat(_name: *const u8) -> isize {
    trace!(
        "kernel:pid[{}] sys_unlinkat",
        current_task().unwrap().pid.0
    );

    let token = current_user_token();
    let name = translated_str(token, _name);

    if unlinkat(&name) { 0 } else { -1 }
}
