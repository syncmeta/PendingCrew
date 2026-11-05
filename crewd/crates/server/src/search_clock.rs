//! Trusted, process-independent query lifetime. Continuous boot time includes
//! sleep and survives Core restarts; a native boot identity fences OS reboots.
use crewd_collab::organization_search::ReadClock;
use crewd_collab::{
    CollabError,
    refusal::{Code, Refusal},
};
use std::io;

pub(crate) fn read() -> crewd_collab::Result<ReadClock> {
    let boot_id = crewd_exec::registry::recovery::boot_id().map_err(|_| unavailable())?;
    let elapsed_ms = elapsed_ms().map_err(|_| unavailable())?;
    Ok(ReadClock {
        wall_ms: crewd_store::now_ms(),
        elapsed_ms,
        boot_id,
    })
}

fn unavailable() -> CollabError {
    CollabError::Conflict(Refusal::new(Code::SearchUnavailable))
}

#[cfg(target_os = "macos")]
fn elapsed_ms() -> io::Result<u64> {
    #[repr(C)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }
    unsafe extern "C" {
        fn mach_timebase_info(info: *mut Timebase) -> libc::c_int;
        fn mach_continuous_time() -> u64;
    }
    let mut info = Timebase { numer: 0, denom: 0 };
    // SAFETY: the writable repr(C) buffer exactly matches mach_timebase_info_data_t.
    if unsafe { mach_timebase_info(&mut info) } != 0 || info.numer == 0 || info.denom == 0 {
        return Err(io::Error::other("continuous clock timebase unavailable"));
    }
    // SAFETY: this read-only API takes no pointers and is available since macOS 10.12.
    let ticks = unsafe { mach_continuous_time() };
    let millis = u128::from(ticks)
        .checked_mul(u128::from(info.numer))
        .ok_or_else(|| io::Error::other("continuous clock overflow"))?
        / u128::from(info.denom)
        / 1_000_000;
    u64::try_from(millis).map_err(|_| io::Error::other("continuous clock range"))
}

#[cfg(target_os = "linux")]
fn elapsed_ms() -> io::Result<u64> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: the timespec is a valid writable buffer; CLOCK_BOOTTIME is read-only.
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut time) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let seconds =
        u64::try_from(time.tv_sec).map_err(|_| io::Error::other("continuous clock seconds"))?;
    let nanos = u64::try_from(time.tv_nsec)
        .ok()
        .filter(|value| *value < 1_000_000_000)
        .ok_or_else(|| io::Error::other("continuous clock nanoseconds"))?;
    seconds
        .checked_mul(1000)
        .and_then(|value| value.checked_add(nanos / 1_000_000))
        .ok_or_else(|| io::Error::other("continuous clock overflow"))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn elapsed_ms() -> io::Result<u64> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "continuous boot clock unavailable",
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn kernel_clock_is_boot_bound_and_does_not_go_backwards() {
        let first = super::read().unwrap();
        let second = super::read().unwrap();
        assert!(!first.boot_id.is_empty());
        assert_eq!(first.boot_id, second.boot_id);
        assert!(second.elapsed_ms >= first.elapsed_ms);
    }
}
