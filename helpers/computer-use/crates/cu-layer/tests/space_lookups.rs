//! The one-window Space check (patches/0009) asks the window server about
//! that window only and answers what the whole window list says about it.
//! Reads the live window list; needs no permission.

use platform_macos::windows::{all_windows, window_on_current_space_by_id};

#[test]
fn one_window_space_check_matches_the_whole_list() {
    let windows = all_windows();
    // The user can switch Spaces between the two reads: a mismatch is read
    // once more before it counts.
    let mismatches: Vec<u32> = windows
        .iter()
        .take(40)
        .filter(|w| window_on_current_space_by_id(w.window_id) != w.on_current_space)
        .filter(|w| {
            let again = all_windows()
                .into_iter()
                .find(|other| other.window_id == w.window_id);
            again.is_some_and(|again| {
                window_on_current_space_by_id(w.window_id) != again.on_current_space
            })
        })
        .map(|w| w.window_id)
        .collect();
    assert!(
        mismatches.is_empty(),
        "windows answered differently: {mismatches:?}"
    );
}

#[test]
fn an_unknown_window_has_no_space_answer() {
    assert_eq!(window_on_current_space_by_id(u32::MAX), None);
}
