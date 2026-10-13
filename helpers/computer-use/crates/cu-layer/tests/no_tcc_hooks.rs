//! A test process never captures the screen, also not for the activity
//! journal's frame after an action. Its own file, so the environment is set
//! before anything in the process reads it.

#[test]
fn a_test_process_takes_no_frames() {
    std::env::set_var(cu_layer::no_tcc::NO_TCC_FOR_TESTS_ENV, "1");
    cu_layer::registry::install_recording_hooks();
    assert!(cua_driver_core::recording::screenshot_for(Some(1), Some(1)).is_none());
    assert!(cua_driver_core::recording::screenshot_for(None, None).is_none());
}
