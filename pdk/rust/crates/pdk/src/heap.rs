//! ABI v0 专用堆分配器：一块 2MB 静态内存上的单线程 first-fit 空闲链表，
//! 仅用于跨边界 ABI 缓冲区（字符串）。插件内部数据走 Rust 自带分配器，互不相干。
//!
//! 块布局（8 字节对齐）：
//!   [u32 size_with_flag][u32 保留][payload ...]
//!   size_with_flag 的 bit0 = 是否已分配，其余位 = payload 字节数（8 对齐）。

const HEAP_SIZE: usize = 2 * 1024 * 1024;
const HEADER: usize = 8;
const MIN_SPLIT: usize = 32;

#[repr(align(8))]
struct AlignedHeap(#[allow(dead_code)] [u8; HEAP_SIZE]);

static mut HEAP: AlignedHeap = AlignedHeap([0; HEAP_SIZE]);

#[inline]
fn align8(n: usize) -> usize {
    (n + 7) & !7
}

#[inline]
unsafe fn block_info(p: *mut u8) -> (usize, bool) {
    let raw = (p as *mut u32).read();
    ((raw & !1) as usize, raw & 1 == 1)
}

#[inline]
unsafe fn write_block(p: *mut u8, payload: usize, used: bool) {
    (p as *mut u32).write((payload as u32) | used as u32);
}

#[inline]
unsafe fn heap_base() -> *mut u8 {
    std::ptr::addr_of_mut!(HEAP) as *mut u8
}

unsafe fn carve(p: *mut u8, avail: usize, want: usize) -> *mut u8 {
    if avail >= want + MIN_SPLIT {
        write_block(p, want, true);
        let next = p.add(HEADER + want);
        write_block(next, avail - want - HEADER, false);
    } else {
        write_block(p, avail, true);
    }
    p.add(HEADER)
}

/// 分配 `len` 字节（未清零）。失败返回空指针。
pub unsafe fn alloc_bytes(len: usize) -> *mut u8 {
    let want = align8(len.max(1));
    let base = heap_base();
    let mut off = 0usize;
    while off + HEADER <= HEAP_SIZE {
        let p = base.add(off);
        let (size, used) = block_info(p);
        if size == 0 {
            // 未开垦区域：到堆尾为止的一整块空闲块
            let avail = HEAP_SIZE - off - HEADER;
            return if avail >= want {
                carve(p, avail, want)
            } else {
                std::ptr::null_mut()
            };
        }
        if !used && size >= want {
            return carve(p, size, want);
        }
        off += HEADER + size;
    }
    std::ptr::null_mut()
}

/// 释放由 `alloc_bytes` 返回的指针。`len` 参数仅为 ABI 兼容保留。
pub unsafe fn dealloc(ptr: *mut u8, _len: usize) {
    if ptr.is_null() {
        return;
    }
    let base = heap_base();
    let mut off = 0usize;
    while off + HEADER <= HEAP_SIZE {
        let p = base.add(off);
        let (size, used) = block_info(p);
        if size == 0 {
            return; // 未找到该指针（重复释放或越界），静默忽略
        }
        if p.add(HEADER) == ptr && used {
            write_block(p, size, false);
            // 向前合并所有连续空闲块
            let mut cur = size;
            loop {
                let next_off = off + HEADER + cur;
                if next_off + HEADER > HEAP_SIZE {
                    break;
                }
                let np = base.add(next_off);
                let (nsize, nused) = block_info(np);
                if nsize == 0 {
                    // 与未开垦区合并
                    write_block(p, HEAP_SIZE - off - HEADER, false);
                    break;
                }
                if nused {
                    break;
                }
                cur += HEADER + nsize;
                write_block(p, cur, false);
            }
            return;
        }
        off += HEADER + size;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_write_free_reuse() {
        unsafe {
            let a = alloc_bytes(100);
            assert!(!a.is_null());
            a.write_bytes(0xAB, 100);
            let b = alloc_bytes(200);
            assert!(!b.is_null());
            assert_ne!(a, b);
            assert_eq!(a.read(), 0xAB);
            dealloc(a, 100);
            // 释放后应能复用（first-fit）
            let c = alloc_bytes(50);
            assert_eq!(c, a);
            dealloc(b, 200);
            dealloc(c, 50);
        }
    }
}
