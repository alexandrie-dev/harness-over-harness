use super::*;
use std::cell::RefCell;

struct Flags {
    calls: RefCell<Vec<(u64, u8, u32)>>,
    failing: Option<u8>,
}
impl Flags {
    fn call(&self, fd: u64, cmd: u8, arg: u32) -> Result<(), BoxError> {
        self.calls.borrow_mut().push((fd, cmd, arg));
        if self.failing == Some(cmd) {
            Err(system_error(
                std::io::Error::from_raw_os_error(9),
                "fcntl",
                None,
            ))
        } else {
            Ok(())
        }
    }
}
impl DescriptorFlags for Flags {
    fn getfd(&self, fd: u64) -> Result<FdFlags, BoxError> {
        self.call(fd, 1, 0)?;
        let flags = if (self.failing == Some(2)) || (self.failing.is_none() && fd != 1) {
            FdFlags::CLOEXEC
        } else {
            FdFlags::empty()
        };
        Ok(if self.failing.is_none() && fd == 2 {
            flags | FdFlags::from_bits_retain(4)
        } else {
            flags
        })
    }
    fn setfd(&self, fd: u64, flags: FdFlags) -> Result<(), BoxError> {
        self.call(fd, 2, flags.bits())
    }
    fn getfl(&self, fd: u64) -> Result<OFlags, BoxError> {
        self.call(fd, 3, 0)?;
        Ok(if fd == 2 {
            OFlags::WRONLY
        } else {
            OFlags::NONBLOCK | OFlags::RDWR
        })
    }
    fn setfl(&self, fd: u64, flags: OFlags) -> Result<(), BoxError> {
        self.call(fd, 4, flags.bits())
    }
}
#[test]
fn preserves_inherited_stdio_with_and_without_control() {
    for control in [false, true] {
        let flags = Flags {
            calls: RefCell::default(),
            failing: None,
        };
        prepare_descriptors(&flags, control, &[0, 2]).unwrap();
        let mut expected = vec![(0, 1, 0), (0, 2, 0), (1, 1, 0), (2, 1, 0), (2, 2, 4)];
        if control {
            expected.extend([(7, 1, 0), (7, 2, 0)]);
        }
        expected.extend([(0, 3, 0), (0, 4, OFlags::RDWR.bits()), (2, 3, 0)]);
        assert_eq!(*flags.calls.borrow(), expected);
    }
    let failure = system_error(
        std::io::Error::from_raw_os_error(2),
        "execve",
        Some("/missing/tool"),
    );
    let record = failure
        .downcast_ref::<crate::runner_protocol::RunnerFailure>()
        .unwrap()
        .diagnostic();
    assert_eq!(record.code.as_deref(), Some("ENOENT"));
    assert_eq!(record.syscall.as_deref(), Some("execve"));
    assert_eq!(record.path.as_deref(), Some("/missing/tool"));
    assert_eq!(
        record.message,
        "ENOENT: no such file or directory, execve '/missing/tool'"
    );
}
#[test]
fn stops_before_execve_when_fcntl_fails() {
    for (command, expected) in [
        (1, vec![(0, 1, 0)]),
        (2, vec![(0, 1, 0), (0, 2, 0)]),
        (3, vec![(0, 1, 0), (1, 1, 0), (2, 1, 0), (0, 3, 0)]),
        (
            4,
            vec![
                (0, 1, 0),
                (1, 1, 0),
                (2, 1, 0),
                (0, 3, 0),
                (0, 4, OFlags::RDWR.bits()),
            ],
        ),
    ] {
        let flags = Flags {
            calls: RefCell::default(),
            failing: Some(command),
        };
        let failure = prepare_descriptors(&flags, false, &[0]).unwrap_err();
        assert_eq!(*flags.calls.borrow(), expected);
        let record = failure
            .downcast_ref::<crate::runner_protocol::RunnerFailure>()
            .unwrap()
            .diagnostic();
        assert_eq!(record.code.as_deref(), Some("EBADF"));
        assert_eq!(record.syscall.as_deref(), Some("fcntl"));
    }
}
