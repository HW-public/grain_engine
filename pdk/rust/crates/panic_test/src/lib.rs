//! panic/trap 行为验证小程序（宿主 harness 的可选导出）：
//! - `spin()`  死循环，配合 fuel 上限观察耗尽 trap
//! - `panic()` 故意 panic，观察 trap 行为
//!
//! 运行期观测由宿主完成；release 下 panic = "abort" → unreachable → trap。

/// fuel 判决用：永不返回的死循环。fuel 耗尽后宿主应观察到 trap。
#[no_mangle]
pub extern "C" fn spin() {
    loop {
        std::hint::spin_loop();
    }
}

/// panic 判决用：立即 panic（release panic=abort → wasm unreachable）。
#[no_mangle]
pub extern "C" fn panic() {
    core::panic!("intentional panic: wasm trap observability test");
}

#[no_mangle]
pub extern "C" fn add(a: i32, b: i32) -> i32 {
    a + b
}
