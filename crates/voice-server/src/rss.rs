pub fn resident_bytes() -> u64 {
    #[cfg(target_os = "linux")]
    if let Ok(stat) = std::fs::read_to_string("/proc/self/statm") {
        if let Some(pages) = stat.split_whitespace().nth(1) {
            return pages.parse::<u64>().unwrap_or(0) * 4096;
        }
    }
    #[cfg(target_os = "macos")]
    #[allow(deprecated)]
    unsafe {
        let mut info: libc::mach_task_basic_info = std::mem::zeroed();
        let mut count = (std::mem::size_of_val(&info) / 4) as libc::mach_msg_type_number_t;
        if libc::task_info(
            libc::mach_task_self(),
            libc::MACH_TASK_BASIC_INFO,
            &mut info as *mut _ as *mut _,
            &mut count,
        ) == libc::KERN_SUCCESS
        {
            return info.resident_size;
        }
    }
    0
}
