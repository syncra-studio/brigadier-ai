//! WebView2 capture. COM objects stay on the UI thread; only the PNG bytes cross back.
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::rc::Rc;

use brigadier_ipc::protocol::{ErrorCode, IpcError};
use tokio::sync::oneshot::Sender;
use webview2_com::CapturePreviewCompletedHandler;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG, ICoreWebView2Controller,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::StructuredStorage::CreateStreamOnHGlobal;
use windows::Win32::System::Com::{IStream, STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET};
use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;
use wry::WebViewExtWindows;

fn failed(error: windows::core::Error) -> IpcError {
    IpcError {
        code: ErrorCode::Internal,
        message: format!("the page's picture couldn't be taken: {error}"),
    }
}

/// Reads exactly the bytes WebView2 wrote; the stream's cursor is at its end after capture.
fn bytes(stream: &IStream) -> windows::core::Result<Vec<u8>> {
    // SAFETY: the stream is live on its owning UI thread. Stat writes to a valid struct,
    // Seek has no output pointer, and Read receives an allocated buffer of the stated size.
    unsafe {
        let mut stat = STATSTG::default();
        stream.Stat(&mut stat, STATFLAG_NONAME)?;
        // Bound an unexpected stream size before allocating or narrowing it to Read's u32.
        if stat.cbSize == 0 || stat.cbSize > 64 * 1024 * 1024 {
            return Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_INVALIDARG,
            ));
        }
        stream.Seek(0, STREAM_SEEK_SET, None)?;
        let mut png = vec![0; stat.cbSize as usize];
        let mut read = 0;
        stream
            .Read(png.as_mut_ptr().cast(), png.len() as u32, Some(&mut read))
            .ok()?;
        if read as usize != png.len() {
            return Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_FAIL,
            ));
        }
        Ok(png)
    }
}

/// A hidden tab's page draws nothing, so its capture would never complete: for the capture
/// WebView2 is told the page shows, while the tab's window stays hidden, so nothing shows or
/// moves on screen. Hidden again after, unless the tab was shown meanwhile.
fn draw_while_hidden(
    controller: &ICoreWebView2Controller,
) -> windows::core::Result<Option<ICoreWebView2Controller>> {
    // SAFETY: wry owns the live controller on this UI thread; IsVisible writes to a valid BOOL.
    unsafe {
        let mut drawn = windows::core::BOOL::default();
        controller.IsVisible(&mut drawn)?;
        if drawn.as_bool() {
            return Ok(None);
        }
        controller.SetIsVisible(true)?;
    }
    Ok(Some(controller.clone()))
}

fn hide_again(controller: &ICoreWebView2Controller) {
    // SAFETY: as above; ParentWindow writes the tab's own window, which wry shows and hides
    // with the page.
    unsafe {
        let mut window = HWND::default();
        if controller.ParentWindow(&mut window).is_ok() && !IsWindowVisible(window).as_bool() {
            let _ = controller.SetIsVisible(false);
        }
    }
}

pub(super) fn capture(webview: &wry::WebView, sent: Sender<Result<Vec<u8>, IpcError>>) {
    let sent = Rc::new(RefCell::new(Some(sent)));
    let reply = sent.clone();
    let hidden = Rc::new(RefCell::new(None));
    let shown_for = hidden.clone();
    let start = || -> windows::core::Result<()> {
        // SAFETY: null asks COM to allocate the stream's storage; it is freed on release.
        let stream = unsafe { CreateStreamOnHGlobal(Default::default(), true)? };
        let captured = stream.clone();
        let handler = CapturePreviewCompletedHandler::create(Box::new(move |result| {
            if let Some(controller) = shown_for.borrow_mut().take() {
                hide_again(&controller);
            }
            let png = result.and_then(|()| bytes(&captured)).map_err(failed);
            if let Some(sent) = reply.borrow_mut().take() {
                let _ = sent.send(png);
            }
            Ok(())
        }));
        *hidden.borrow_mut() = draw_while_hidden(&webview.controller())?;
        // SAFETY: wry owns this live WebView2 on the UI thread. WebView2 retains the stream
        // and callback through completion; the callback also retains its own stream reference.
        unsafe {
            webview.webview().CapturePreview(
                COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
                &stream,
                &handler,
            )
        }
    };
    if let Err(error) = start() {
        if let Some(controller) = hidden.borrow_mut().take() {
            hide_again(&controller);
        }
        if let Some(sent) = sent.borrow_mut().take() {
            let _ = sent.send(Err(failed(error)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_reads_from_the_start_of_the_stream() {
        // SAFETY: this stream owns its allocation; Write gets a valid slice and length.
        unsafe {
            let stream = CreateStreamOnHGlobal(Default::default(), true).unwrap();
            let png = b"\x89PNG\r\n\x1a\nimage";
            stream
                .Write(png.as_ptr().cast(), png.len() as u32, None)
                .ok()
                .unwrap();
            assert_eq!(bytes(&stream).unwrap(), png);
        }
    }

    #[test]
    fn empty_or_oversized_capture_is_refused() {
        // SAFETY: SetSize changes only the size of our own stream.
        unsafe {
            let stream = CreateStreamOnHGlobal(Default::default(), true).unwrap();
            assert!(bytes(&stream).is_err());
            stream.SetSize(64 * 1024 * 1024 + 1).unwrap();
            assert!(bytes(&stream).is_err());
        }
    }
}
