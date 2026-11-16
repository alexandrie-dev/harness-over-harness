//! Node/libuv system-error names and texts, independent of libc's Display.
use std::io;

/// The OS error's Node name, or `None` for unnamed errors and non-Unix hosts.
pub fn node_error_code(error: &io::Error) -> Option<&'static str> {
    #[cfg(unix)]
    {
        use rustix::io::Errno;
        // Compare against named constants without constructing Errno: arbitrary
        // io::Error numbers can fall outside Linux's reserved errno range.
        let errno = error.raw_os_error()?;
        let codes = [
            (Errno::TOOBIG, "E2BIG"),
            (Errno::ACCESS, "EACCES"),
            (Errno::ADDRINUSE, "EADDRINUSE"),
            (Errno::ADDRNOTAVAIL, "EADDRNOTAVAIL"),
            (Errno::AFNOSUPPORT, "EAFNOSUPPORT"),
            (Errno::AGAIN, "EAGAIN"),
            (Errno::ALREADY, "EALREADY"),
            (Errno::BADF, "EBADF"),
            (Errno::BUSY, "EBUSY"),
            (Errno::CANCELED, "ECANCELED"),
            (Errno::CONNABORTED, "ECONNABORTED"),
            (Errno::CONNREFUSED, "ECONNREFUSED"),
            (Errno::CONNRESET, "ECONNRESET"),
            (Errno::DESTADDRREQ, "EDESTADDRREQ"),
            (Errno::EXIST, "EEXIST"),
            (Errno::FAULT, "EFAULT"),
            (Errno::FBIG, "EFBIG"),
            (Errno::HOSTUNREACH, "EHOSTUNREACH"),
            (Errno::INTR, "EINTR"),
            (Errno::INVAL, "EINVAL"),
            (Errno::IO, "EIO"),
            (Errno::ISCONN, "EISCONN"),
            (Errno::ISDIR, "EISDIR"),
            (Errno::LOOP, "ELOOP"),
            (Errno::MFILE, "EMFILE"),
            (Errno::MSGSIZE, "EMSGSIZE"),
            (Errno::NAMETOOLONG, "ENAMETOOLONG"),
            (Errno::NETDOWN, "ENETDOWN"),
            (Errno::NETUNREACH, "ENETUNREACH"),
            (Errno::NFILE, "ENFILE"),
            (Errno::NOBUFS, "ENOBUFS"),
            (Errno::NODEV, "ENODEV"),
            (Errno::NOENT, "ENOENT"),
            (Errno::NOMEM, "ENOMEM"),
            #[cfg(not(any(
                target_vendor = "apple",
                target_os = "freebsd",
                target_os = "dragonfly",
                target_os = "netbsd",
                target_os = "openbsd",
                target_os = "aix",
                target_os = "espidf",
                target_os = "haiku",
                target_os = "horizon",
                target_os = "hurd",
                target_os = "l4re",
                target_os = "vita",
                target_os = "wasi",
            )))]
            (Errno::NONET, "ENONET"),
            (Errno::NOPROTOOPT, "ENOPROTOOPT"),
            (Errno::NOSPC, "ENOSPC"),
            (Errno::NOSYS, "ENOSYS"),
            (Errno::NOTCONN, "ENOTCONN"),
            (Errno::NOTDIR, "ENOTDIR"),
            (Errno::NOTEMPTY, "ENOTEMPTY"),
            (Errno::NOTSOCK, "ENOTSOCK"),
            (Errno::NOTSUP, "ENOTSUP"),
            (Errno::OVERFLOW, "EOVERFLOW"),
            (Errno::PERM, "EPERM"),
            (Errno::PIPE, "EPIPE"),
            (Errno::PROTO, "EPROTO"),
            (Errno::PROTONOSUPPORT, "EPROTONOSUPPORT"),
            (Errno::PROTOTYPE, "EPROTOTYPE"),
            (Errno::RANGE, "ERANGE"),
            (Errno::ROFS, "EROFS"),
            (Errno::SHUTDOWN, "ESHUTDOWN"),
            (Errno::SPIPE, "ESPIPE"),
            (Errno::SRCH, "ESRCH"),
            (Errno::TIMEDOUT, "ETIMEDOUT"),
            (Errno::TXTBSY, "ETXTBSY"),
            (Errno::XDEV, "EXDEV"),
            (Errno::NXIO, "ENXIO"),
            (Errno::MLINK, "EMLINK"),
            (Errno::HOSTDOWN, "EHOSTDOWN"),
            #[cfg(not(any(
                target_vendor = "apple",
                target_os = "freebsd",
                target_os = "dragonfly",
                target_os = "netbsd",
                target_os = "openbsd",
                target_os = "solaris",
                target_os = "illumos",
                target_os = "aix",
                target_os = "cygwin",
                target_os = "espidf",
                target_os = "haiku",
                target_os = "horizon",
                target_os = "hurd",
                target_os = "l4re",
                target_os = "nto",
                target_os = "vita",
                target_os = "wasi",
            )))]
            (Errno::REMOTEIO, "EREMOTEIO"),
            (Errno::NOTTY, "ENOTTY"),
            (Errno::ILSEQ, "EILSEQ"),
            (Errno::SOCKTNOSUPPORT, "ESOCKTNOSUPPORT"),
            #[cfg(not(any(
                target_os = "freebsd",
                target_os = "dragonfly",
                target_os = "haiku",
                target_os = "openbsd"
            )))]
            (Errno::NODATA, "ENODATA"),
            #[cfg(not(any(
                target_vendor = "apple",
                target_os = "freebsd",
                target_os = "dragonfly",
                target_os = "netbsd",
                target_os = "openbsd",
                target_os = "espidf",
                target_os = "haiku",
                target_os = "horizon",
                target_os = "hurd",
                target_os = "l4re",
                target_os = "vita",
                target_os = "wasi",
            )))]
            (Errno::UNATCH, "EUNATCH"),
            (Errno::NOEXEC, "ENOEXEC"),
            #[cfg(any(
                target_env = "newlib",
                target_os = "macos",
                target_os = "ios",
                target_os = "tvos",
                target_os = "watchos",
                target_os = "visionos",
                target_os = "freebsd",
                target_os = "dragonfly",
                target_os = "netbsd",
                target_os = "openbsd"
            ))]
            (Errno::FTYPE, "EFTYPE"),
        ];
        codes
            .into_iter()
            .find_map(|(number, code)| (number.raw_os_error() == errno).then_some(code))
    }
    #[cfg(not(unix))]
    {
        let _ = error;
        None
    }
}

/// libuv's description of a Node error code (including libuv-only codes).
pub fn node_error_description(code: &str) -> Option<&'static str> {
    match code {
        "E2BIG" => Some("argument list too long"),
        "EACCES" => Some("permission denied"),
        "EADDRINUSE" => Some("address already in use"),
        "EADDRNOTAVAIL" => Some("address not available"),
        "EAFNOSUPPORT" => Some("address family not supported"),
        "EAGAIN" => Some("resource temporarily unavailable"),
        "EAI_ADDRFAMILY" => Some("address family not supported"),
        "EAI_AGAIN" => Some("temporary failure"),
        "EAI_BADFLAGS" => Some("bad ai_flags value"),
        "EAI_BADHINTS" => Some("invalid value for hints"),
        "EAI_CANCELED" => Some("request canceled"),
        "EAI_FAIL" => Some("permanent failure"),
        "EAI_FAMILY" => Some("ai_family not supported"),
        "EAI_MEMORY" => Some("out of memory"),
        "EAI_NODATA" => Some("no address"),
        "EAI_NONAME" => Some("unknown node or service"),
        "EAI_OVERFLOW" => Some("argument buffer overflow"),
        "EAI_PROTOCOL" => Some("resolved protocol is unknown"),
        "EAI_SERVICE" => Some("service not available for socket type"),
        "EAI_SOCKTYPE" => Some("socket type not supported"),
        "EALREADY" => Some("connection already in progress"),
        "EBADF" => Some("bad file descriptor"),
        "EBUSY" => Some("resource busy or locked"),
        "ECANCELED" => Some("operation canceled"),
        "ECHARSET" => Some("invalid Unicode character"),
        "ECONNABORTED" => Some("software caused connection abort"),
        "ECONNREFUSED" => Some("connection refused"),
        "ECONNRESET" => Some("connection reset by peer"),
        "EDESTADDRREQ" => Some("destination address required"),
        "EEXIST" => Some("file already exists"),
        "EFAULT" => Some("bad address in system call argument"),
        "EFBIG" => Some("file too large"),
        "EHOSTUNREACH" => Some("host is unreachable"),
        "EINTR" => Some("interrupted system call"),
        "EINVAL" => Some("invalid argument"),
        "EIO" => Some("i/o error"),
        "EISCONN" => Some("socket is already connected"),
        "EISDIR" => Some("illegal operation on a directory"),
        "ELOOP" => Some("too many symbolic links encountered"),
        "EMFILE" => Some("too many open files"),
        "EMSGSIZE" => Some("message too long"),
        "ENAMETOOLONG" => Some("name too long"),
        "ENETDOWN" => Some("network is down"),
        "ENETUNREACH" => Some("network is unreachable"),
        "ENFILE" => Some("file table overflow"),
        "ENOBUFS" => Some("no buffer space available"),
        "ENODEV" => Some("no such device"),
        "ENOENT" => Some("no such file or directory"),
        "ENOMEM" => Some("not enough memory"),
        "ENONET" => Some("machine is not on the network"),
        "ENOPROTOOPT" => Some("protocol not available"),
        "ENOSPC" => Some("no space left on device"),
        "ENOSYS" => Some("function not implemented"),
        "ENOTCONN" => Some("socket is not connected"),
        "ENOTDIR" => Some("not a directory"),
        "ENOTEMPTY" => Some("directory not empty"),
        "ENOTSOCK" => Some("socket operation on non-socket"),
        "ENOTSUP" => Some("operation not supported on socket"),
        "EOVERFLOW" => Some("value too large for defined data type"),
        "EPERM" => Some("operation not permitted"),
        "EPIPE" => Some("broken pipe"),
        "EPROTO" => Some("protocol error"),
        "EPROTONOSUPPORT" => Some("protocol not supported"),
        "EPROTOTYPE" => Some("protocol wrong type for socket"),
        "ERANGE" => Some("result too large"),
        "EROFS" => Some("read-only file system"),
        "ESHUTDOWN" => Some("cannot send after transport endpoint shutdown"),
        "ESPIPE" => Some("invalid seek"),
        "ESRCH" => Some("no such process"),
        "ETIMEDOUT" => Some("connection timed out"),
        "ETXTBSY" => Some("text file is busy"),
        "EXDEV" => Some("cross-device link not permitted"),
        "UNKNOWN" => Some("unknown error"),
        "EOF" => Some("end of file"),
        "ENXIO" => Some("no such device or address"),
        "EMLINK" => Some("too many links"),
        "EHOSTDOWN" => Some("host is down"),
        "EREMOTEIO" => Some("remote I/O error"),
        "ENOTTY" => Some("inappropriate ioctl for device"),
        "EFTYPE" => Some("inappropriate file type or format"),
        "EILSEQ" => Some("illegal byte sequence"),
        "ESOCKTNOSUPPORT" => Some("socket type not supported"),
        "ENODATA" => Some("no data available"),
        "EUNATCH" => Some("protocol driver not attached"),
        "ENOEXEC" => Some("exec format error"),
        _ => None,
    }
}

fn error_text(error: &io::Error) -> String {
    match node_error_code(error)
        .and_then(|code| node_error_description(code).map(|text| (code, text)))
    {
        Some((code, text)) => format!("{code}: {text}"),
        None => error.to_string(),
    }
}

/// Format a Node fs system error using the caller's syscall and optional paths.
/// Unnamed errors use their own Display text instead of code and description.
pub fn node_fs_error_message(
    error: &io::Error,
    syscall: &str,
    path: Option<&str>,
    dest: Option<&str>,
) -> String {
    let mut message = format!("{}, {syscall}", error_text(error));
    if let Some(path) = path {
        message.push_str(&format!(" '{path}'"));
    }
    if let Some(dest) = dest {
        message.push_str(&format!(" -> '{dest}'"));
    }
    message
}

/// Format a failed Node child-process start, including the command for five codes.
/// Unnamed errors use their own Display text after `spawn`.
pub fn node_spawn_error_message(error: &io::Error, command: &str) -> String {
    match node_error_code(error) {
        Some(code @ ("EACCES" | "EAGAIN" | "EMFILE" | "ENFILE" | "ENOENT")) => {
            format!("spawn {command} {code}")
        }
        Some(code) => format!("spawn {code}"),
        None => format!("spawn {error}"),
    }
}
