use objc2::{
    Encode, ffi,
    runtime::{AnyClass, AnyObject, AnyProtocol, Bool, Imp, ProtocolBuilder, Sel},
};
use std::sync::{
    OnceLock,
    atomic::{AtomicBool, Ordering},
};

static HANDLING_SEND_EVENT: AtomicBool = AtomicBool::new(false);
static ORIGINAL_SEND_EVENT: OnceLock<usize> = OnceLock::new();

extern "C-unwind" fn set_handling_send_event(
    _this: *mut AnyObject,
    _selector: Sel,
    handling: Bool,
) {
    HANDLING_SEND_EVENT.store(handling.as_bool(), Ordering::Release);
}

extern "C-unwind" fn is_handling_send_event(_this: *mut AnyObject, _selector: Sel) -> Bool {
    Bool::new(HANDLING_SEND_EVENT.load(Ordering::Acquire))
}

extern "C-unwind" fn send_event(this: *mut AnyObject, selector: Sel, event: *mut AnyObject) {
    let nested = HANDLING_SEND_EVENT.swap(true, Ordering::AcqRel);
    if let Some(original) = ORIGINAL_SEND_EVENT.get().copied() {
        let original: extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) =
            unsafe { std::mem::transmute(original) };
        original(this, selector, event);
    }
    if !nested {
        HANDLING_SEND_EVENT.store(false, Ordering::Release);
    }
}

unsafe fn method(function: *const ()) -> Imp {
    unsafe { std::mem::transmute(function) }
}

/// Add CEF's application contract to Tao's already-created application class.
/// Wrapping Tao's existing `sendEvent:` implementation preserves its Command
/// key-up and raw-device-event forwarding instead of replacing it with the
/// smaller cefsimple implementation.
pub(super) fn install() -> Result<(), String> {
    let class = AnyClass::get(c"TaoApp")
        .ok_or_else(|| "Tao NSApplication class is not initialized".to_string())?;
    let app = AnyProtocol::get(c"CrAppProtocol")
        .or_else(|| {
            let mut protocol = ProtocolBuilder::new(c"CrAppProtocol")?;
            protocol
                .add_method_description::<(), Bool>(Sel::register(c"isHandlingSendEvent"), true);
            Some(protocol.register())
        })
        .ok_or_else(|| "CEF app protocol could not be registered".to_string())?;
    let control = AnyProtocol::get(c"CrAppControlProtocol")
        .or_else(|| {
            let mut protocol = ProtocolBuilder::new(c"CrAppControlProtocol")?;
            protocol.add_protocol(app);
            protocol.add_method_description::<(Bool,), ()>(
                Sel::register(c"setHandlingSendEvent:"),
                true,
            );
            Some(protocol.register())
        })
        .ok_or_else(|| "CEF control protocol could not be registered".to_string())?;
    let cef = AnyProtocol::get(c"CefAppProtocol")
        .or_else(|| {
            let mut protocol = ProtocolBuilder::new(c"CefAppProtocol")?;
            protocol.add_protocol(control);
            Some(protocol.register())
        })
        .ok_or_else(|| "CEF application protocol could not be registered".to_string())?;

    let raw_class = (class as *const AnyClass).cast_mut();
    let setter_types = std::ffi::CString::new(format!("v@:{}", Bool::ENCODING))
        .map_err(|error| error.to_string())?;
    let getter_types = std::ffi::CString::new(format!("{}@:", Bool::ENCODING))
        .map_err(|error| error.to_string())?;
    unsafe {
        ffi::class_addProtocol(raw_class, app as *const AnyProtocol);
        ffi::class_addProtocol(raw_class, control as *const AnyProtocol);
        ffi::class_addProtocol(raw_class, cef as *const AnyProtocol);

        let setter = Sel::register(c"setHandlingSendEvent:");
        let getter = Sel::register(c"isHandlingSendEvent");
        if ffi::class_addMethod(
            raw_class,
            setter,
            method(set_handling_send_event as *const ()),
            setter_types.as_ptr(),
        ) == Bool::NO
        {
            return Err("TaoApp already defines setHandlingSendEvent:".to_string());
        }
        if ffi::class_addMethod(
            raw_class,
            getter,
            method(is_handling_send_event as *const ()),
            getter_types.as_ptr(),
        ) == Bool::NO
        {
            return Err("TaoApp already defines isHandlingSendEvent".to_string());
        }

        let send = Sel::register(c"sendEvent:");
        let original = class
            .instance_method(send)
            .ok_or_else(|| "TaoApp sendEvent: implementation is unavailable".to_string())?
            .implementation();
        let previous = ffi::class_replaceMethod(
            raw_class,
            send,
            method(send_event as *const ()),
            c"v@:@".as_ptr(),
        );
        let previous = previous.unwrap_or(original);
        let _ = ORIGINAL_SEND_EVENT.set(previous as usize);
    }
    Ok(())
}
