use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::null;

use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Networking::WinInet::{
    HTTP_QUERY_FLAG_NUMBER, HTTP_QUERY_STATUS_CODE, HttpQueryInfoW, INTERNET_FLAG_NO_CACHE_WRITE,
    INTERNET_FLAG_NO_UI, INTERNET_FLAG_RELOAD, INTERNET_FLAG_SECURE, INTERNET_OPEN_TYPE_PRECONFIG,
    INTERNET_OPTION_CONNECT_TIMEOUT, INTERNET_OPTION_RECEIVE_TIMEOUT, INTERNET_OPTION_SEND_TIMEOUT,
    InternetCloseHandle, InternetOpenUrlW, InternetOpenW, InternetReadFile, InternetSetOptionW,
};

const USER_AGENT: &str = concat!("HiyoDyna/", env!("CARGO_PKG_VERSION"));
const CONNECT_TIMEOUT_MILLISECONDS: u32 = 10_000;
const RECEIVE_TIMEOUT_MILLISECONDS: u32 = 30_000;
const SEND_TIMEOUT_MILLISECONDS: u32 = 30_000;
const READ_BUFFER_BYTES: usize = 64 * 1024;

pub(crate) struct HttpError {
    pub status: Option<u32>,
    pub message: String,
}

pub(crate) fn get(url: &str, max_bytes: usize) -> Result<Vec<u8>, HttpError> {
    let agent = wide(USER_AGENT);
    let session = unsafe {
        InternetOpenW(
            agent.as_ptr(),
            INTERNET_OPEN_TYPE_PRECONFIG,
            null(),
            null(),
            0,
        )
    };
    if session.is_null() {
        return Err(last_error("InternetOpenW"));
    }

    for (option, milliseconds) in [
        (
            INTERNET_OPTION_CONNECT_TIMEOUT,
            CONNECT_TIMEOUT_MILLISECONDS,
        ),
        (
            INTERNET_OPTION_RECEIVE_TIMEOUT,
            RECEIVE_TIMEOUT_MILLISECONDS,
        ),
        (INTERNET_OPTION_SEND_TIMEOUT, SEND_TIMEOUT_MILLISECONDS),
    ] {
        if let Err(error) = set_timeout(session, option, milliseconds) {
            unsafe { InternetCloseHandle(session) };
            return Err(error);
        }
    }

    let result = fetch(session, url, max_bytes);
    unsafe { InternetCloseHandle(session) };
    result
}

fn fetch(session: *const c_void, url: &str, max_bytes: usize) -> Result<Vec<u8>, HttpError> {
    let url = wide(url);
    let flags = INTERNET_FLAG_RELOAD
        | INTERNET_FLAG_SECURE
        | INTERNET_FLAG_NO_CACHE_WRITE
        | INTERNET_FLAG_NO_UI;
    let request = unsafe { InternetOpenUrlW(session, url.as_ptr(), null(), 0, flags, 0) };
    if request.is_null() {
        return Err(last_error("InternetOpenUrlW"));
    }

    let result = fetch_request(request, max_bytes);
    unsafe { InternetCloseHandle(request) };
    result
}

fn fetch_request(request: *const c_void, max_bytes: usize) -> Result<Vec<u8>, HttpError> {
    let status = query_status(request)?;
    if !(200..=299).contains(&status) {
        return Err(HttpError {
            status: Some(status),
            message: format!("HTTP {status}"),
        });
    }

    let mut body = Vec::with_capacity(max_bytes.min(READ_BUFFER_BYTES));
    let mut buffer = [0u8; READ_BUFFER_BYTES];
    loop {
        let mut read = 0u32;
        let ok = unsafe {
            InternetReadFile(
                request,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut read,
            )
        };
        if ok == 0 {
            return Err(last_error("InternetReadFile"));
        }
        if read == 0 {
            return Ok(body);
        }
        if body.len() + read as usize > max_bytes {
            return Err(HttpError {
                status: None,
                message: format!("response exceeds {max_bytes} bytes"),
            });
        }
        body.extend_from_slice(&buffer[..read as usize]);
    }
}

fn query_status(request: *const c_void) -> Result<u32, HttpError> {
    let mut status = 0u32;
    let mut length = size_of::<u32>() as u32;
    let mut index = 0u32;
    let ok = unsafe {
        HttpQueryInfoW(
            request,
            HTTP_QUERY_STATUS_CODE | HTTP_QUERY_FLAG_NUMBER,
            (&mut status as *mut u32).cast(),
            &mut length,
            &mut index,
        )
    };
    if ok == 0 {
        return Err(last_error("HttpQueryInfoW"));
    }
    Ok(status)
}

fn set_timeout(handle: *const c_void, option: u32, milliseconds: u32) -> Result<(), HttpError> {
    let ok = unsafe {
        InternetSetOptionW(
            handle,
            option,
            (&milliseconds as *const u32).cast(),
            size_of::<u32>() as u32,
        )
    };
    if ok == 0 {
        return Err(last_error("InternetSetOptionW"));
    }
    Ok(())
}

fn last_error(operation: &str) -> HttpError {
    let code = unsafe { GetLastError() };
    HttpError {
        status: None,
        message: format!("{operation} failed with Win32 error {code}"),
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn rejects_an_invalid_url_without_a_status() {
        let error = super::get("not-a-url", 1024).unwrap_err();
        assert_eq!(error.status, None);
        assert!(!error.message.is_empty());
    }
}
