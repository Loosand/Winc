//! 在 `winit` 转成 `WindowEvent::MouseInput` 之前保留 `AppKit` 的左键连击次数。

use std::cell::Cell;

use super::*;

type MouseDown = extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject);

static ORIGINAL: OnceLock<Option<MouseDown>> = OnceLock::new();

thread_local! {
    static LEFT_CLICK_COUNT: Cell<Option<u8>> = const { Cell::new(None) };
}

/// 在窗口内容视图上捕获 `mouseDown:` 的次数。各窗口共用同一个 `winit` 视图类。
pub fn capture_left_click_count(window: &Window) {
    let Some(view) = ns_view(window) else {
        return;
    };
    // SAFETY: 在主线程调用，`view` 属于仍然存活的窗口。`MouseDown` 的签名与 `mouseDown:`
    // 一致：返回 void，参数为 self、选择器和 NSEvent 指针。使用原方法的类型编码，只修改内容视图类。
    ORIGINAL.get_or_init(|| unsafe {
        let class = (view.class() as *const AnyClass).cast_mut();
        let selector = sel!(mouseDown:);
        let method = ffi::class_getInstanceMethod(class, selector);
        if method.is_null() {
            return None;
        }
        let original = ffi::method_getImplementation(method)?;
        let imp = std::mem::transmute::<MouseDown, Imp>(mouse_down);
        ffi::class_replaceMethod(class, selector, imp, ffi::method_getTypeEncoding(method));
        Some(std::mem::transmute::<Imp, MouseDown>(original))
    });
}

extern "C-unwind" fn mouse_down(this: *mut AnyObject, cmd: Sel, event: *mut AnyObject) {
    let Some(Some(original)) = ORIGINAL.get() else {
        return;
    };
    // SAFETY: AppKit 在主线程为 `mouseDown:` 传入有效的 NSEvent。判空后读取 NSInteger 属性。
    let count = unsafe { event.as_ref() }.and_then(|event| {
        // SAFETY: `clickCount` 是鼠标按下事件公开的只读属性，返回 NSInteger。
        let count: isize = unsafe { msg_send![event, clickCount] };
        let previous_clicks = usize::try_from(count).ok()?.checked_sub(1)?;
        // 保留编辑区第三击后重新计数的行为。
        Some((previous_clicks % 3 + 1) as u8)
    });
    LEFT_CLICK_COUNT.with(|slot| {
        let previous = slot.replace(count);
        original(this, cmd, event);
        // 延迟派发的事件使用通用计数，避免取到另一条 AppKit 事件的次数。
        slot.set(previous);
    });
}

/// 取出当前正在派发的左键按下事件的连击次数。
pub fn take_left_click_count() -> Option<u8> {
    LEFT_CLICK_COUNT.with(Cell::take)
}
