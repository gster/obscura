//! Trusted startup binding for Details/Dialog open attributes.
//! Capture each realm's private semantics before author code; never reinstall
//! at Document handoff or read a page-selected prototype/descriptor later.
use deno_core::v8;
use crate::speech_native_bindings::{self as binding, PrivateTarget};

pub(crate) fn install<'s>(
    scope: &mut v8::HandleScope<'s>, initializer: v8::Local<'s, v8::Function>,
) -> Result<(), &'static str> {
    let undefined = v8::undefined(scope);
    let records = initializer.call(scope, undefined.into(), &[])
        .ok_or("HTML open bootstrap failed")?;
    let records = v8::Local::<v8::Array>::try_from(records)
        .map_err(|_| "HTML open bootstrap records")?;
    if records.length() != 2 { return Err("HTML open interface count"); }
    for index in 0..2 {
        let record = records.get_index(scope, index)
            .and_then(|value| v8::Local::<v8::Array>::try_from(value).ok())
            .ok_or("HTML open private interface")?;
        if record.length() != 4 { return Err("HTML open private fields"); }
        let prototype = record.get_index(scope, 0)
            .and_then(|value| v8::Local::<v8::Object>::try_from(value).ok())
            .ok_or("HTML open private prototype")?;
        let getter = record.get_index(scope, 1)
            .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
            .ok_or("HTML open private getter")?;
        let setter = record.get_index(scope, 2)
            .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
            .ok_or("HTML open private setter")?;
        let accepts_receiver = record.get_index(scope, 3)
            .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
            .ok_or("HTML open private brand")?;
        binding::install_attribute(scope, prototype, "open",
            PrivateTarget { semantic: getter, accepts_receiver },
            Some(PrivateTarget { semantic: setter, accepts_receiver }))
            .map_err(|_| "HTML open native attribute installation")?;
    }
    Ok(())
}
