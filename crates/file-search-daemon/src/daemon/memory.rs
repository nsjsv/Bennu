#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub(super) fn release_allocator_idle_pages() {
    // 批次已析构；glibc 的线程安全 trim 会把空闲 arena 归还 cgroup，避免峰值变成常驻内存。
    unsafe {
        libc::malloc_trim(0);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub(super) fn release_allocator_idle_pages() {}
