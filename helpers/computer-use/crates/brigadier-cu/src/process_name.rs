//! A process's name read from the kernel, as `ps -o comm=` shows it. `ps` is
//! setuid, so a sandboxed process can't run it, and every name it gave came
//! back empty there.

/// The name the process `pid` was started under (its `argv[0]`, which
/// `ps -o comm=` shows), per `KERN_PROCARGS2`; `None` for a zombie, another
/// user's process or no such process.
pub(crate) fn process_name(pid: i32) -> Option<String> {
    if pid <= 0 {
        return None;
    }
    let mut max: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    // SAFETY: `max` is a writable `c_int` and `size` says so; the result is checked.
    let read = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            2,
            (&raw mut max).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 || max <= 0 {
        return None;
    }
    let mut buffer = vec![0u8; usize::try_from(max).ok()?];
    let mut size = buffer.len();
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    // SAFETY: `buffer` is writable for `size` bytes and the call writes at most
    // that many, updating `size`; the result is checked before `buffer` is read.
    let read = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 {
        return None;
    }
    buffer.truncate(size);
    argv0(&buffer)
}

/// `argv[0]` from a `KERN_PROCARGS2` buffer: the argument count, the
/// executable's path, the NULs padding it, then the arguments, each ended by
/// a NUL.
fn argv0(buffer: &[u8]) -> Option<String> {
    let count = i32::from_ne_bytes(buffer.get(..4)?.try_into().ok()?);
    let rest = buffer.get(4..)?;
    let path_end = rest.iter().position(|byte| *byte == 0)?;
    let start = path_end + rest[path_end..].iter().position(|byte| *byte != 0)?;
    let name = rest[start..].split(|byte| *byte == 0).next()?;
    (count > 0 && !name.is_empty()).then(|| String::from_utf8_lossy(name).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_process_name_is_the_name_it_was_started_under_until_it_ends() {
        let mut buffer = 2i32.to_ne_bytes().to_vec();
        buffer.extend_from_slice(b"/usr/bin/cargo\0\0\0\0cargo\0test\0HOME=/Users/x\0");
        assert_eq!(argv0(&buffer).as_deref(), Some("cargo"));
        assert_eq!(argv0(&[1, 0]), None);

        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        let name = process_name(pid);
        // Ended before asserting, so a failure leaves no sleep behind.
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(name.as_deref(), Some("/bin/sleep"));
        assert_eq!(process_name(pid), None);
        assert_eq!(process_name(-1), None);
    }
}
