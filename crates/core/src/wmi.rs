//! Minimal read-only WMI queries (string properties only), each on its own thread so COM is
//! initialised the way WMI needs, whatever apartment the caller is in.

use std::collections::HashMap;

use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
    CoInitializeSecurity, CoSetProxyBlanket, CoUninitialize, EOAC_NONE, RPC_C_AUTHN_LEVEL_CALL,
    RPC_C_AUTHN_LEVEL_DEFAULT, RPC_C_IMP_LEVEL_IMPERSONATE,
};
use windows::Win32::System::Variant::{VARIANT, VT_BOOL, VT_BSTR, VariantClear};
use windows::Win32::System::Wmi::{
    IWbemClassObject, IWbemLocator, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY,
    WBEM_INFINITE, WbemLocator,
};
use windows::core::{BSTR, PCWSTR};

use crate::winstr::wide;
use crate::{Error, Result};

const RPC_C_AUTHN_WINNT: u32 = 10;
const RPC_C_AUTHZ_NONE: u32 = 0;
/// Rows read from one query.
const MAX_ROWS: usize = 10_000;

pub type Row = HashMap<String, String>;

/// Runs a WQL query in `ROOT\CIMV2` and returns the requested string properties per row.
pub fn query(wql: &str, props: &[&str]) -> Result<Vec<Row>> {
    let wql = wql.to_string();
    let props: Vec<String> = props.iter().map(|p| p.to_string()).collect();
    std::thread::spawn(move || unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        let r = run(&wql, &props);
        CoUninitialize();
        r
    })
    .join()
    .map_err(|_| Error::Other("WMI query thread panicked".into()))?
}

unsafe fn run(wql: &str, props: &[String]) -> Result<Vec<Row>> {
    unsafe {
        // Fails harmlessly when the process already set security.
        let _ = CoInitializeSecurity(
            None,
            -1,
            None,
            None,
            RPC_C_AUTHN_LEVEL_DEFAULT,
            RPC_C_IMP_LEVEL_IMPERSONATE,
            None,
            EOAC_NONE,
            None,
        );
        let locator: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)?;
        let services = locator.ConnectServer(
            &BSTR::from("ROOT\\CIMV2"),
            &BSTR::new(),
            &BSTR::new(),
            &BSTR::new(),
            0,
            &BSTR::new(),
            None,
        )?;
        CoSetProxyBlanket(
            &services,
            RPC_C_AUTHN_WINNT,
            RPC_C_AUTHZ_NONE,
            PCWSTR::null(),
            RPC_C_AUTHN_LEVEL_CALL,
            RPC_C_IMP_LEVEL_IMPERSONATE,
            None,
            EOAC_NONE,
        )?;
        let rows = services.ExecQuery(
            &BSTR::from("WQL"),
            &BSTR::from(wql),
            WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
            None,
        )?;
        let names: Vec<Vec<u16>> = props.iter().map(wide).collect();
        let mut out = Vec::new();
        while out.len() < MAX_ROWS {
            let mut objs: [Option<IWbemClassObject>; 1] = [None];
            let mut got = 0u32;
            let hr = rows.Next(WBEM_INFINITE, &mut objs, &mut got);
            if hr.is_err() || got == 0 {
                break;
            }
            let Some(obj) = objs[0].take() else { break };
            let mut row = Row::new();
            for (p, w) in props.iter().zip(&names) {
                if let Some(v) = string_prop(&obj, PCWSTR(w.as_ptr())) {
                    row.insert(p.clone(), v);
                }
            }
            out.push(row);
        }
        Ok(out)
    }
}

unsafe fn string_prop(obj: &IWbemClassObject, name: PCWSTR) -> Option<String> {
    unsafe {
        let mut v = VARIANT::default();
        obj.Get(name, 0, &mut v, None, None).ok()?;
        let inner = &v.Anonymous.Anonymous;
        let out = if inner.vt == VT_BSTR {
            Some(inner.Anonymous.bstrVal.to_string())
        } else if inner.vt == VT_BOOL {
            Some((inner.Anonymous.boolVal.0 != 0).to_string())
        } else {
            None
        };
        let _ = VariantClear(&mut v);
        out
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_a_class_any_user_can_see() {
        let rows = super::query("SELECT Caption FROM Win32_OperatingSystem", &["Caption"]).unwrap();
        assert!(
            rows.iter()
                .any(|r| r.get("Caption").is_some_and(|c| c.contains("Windows")))
        );
    }
}
