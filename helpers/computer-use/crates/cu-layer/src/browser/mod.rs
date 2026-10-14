//! Reading and acting on a web page in a browser or Electron app: the
//! browser's debugging port, in-process Apple Events, Electron's inspector
//! and an accessibility page reader, plus opening links in a running browser
//! without bringing it to the front.

pub mod ax_page_reader;
pub mod browser_js;
pub mod cdp_client;
pub mod electron_js;
pub mod main_loop;
pub mod open_link;
pub mod page_core;
pub mod wk_web_view;

pub use ax_page_reader::AXPageReader;
pub use browser_js::BrowserJs;
pub use cdp_client::{CdpClient, CdpSessionCache};
pub use electron_js::ElectronJs;
pub use main_loop::{mark_appkit_main_loop_available, mark_appkit_main_loop_stopped};
pub use wk_web_view::is_wk_web_view_app;
