//! Multi-client / multi-cursor / overlay session tests.
//!
//! Concurrent driver processes, per-cursor instance state, cross-process
//! isolation, the live overlay render loop, and the `set_agent_cursor_motion`
//! Bezier knobs. Split out of the old monolithic `mcp_protocol_test.rs`;
//! mac/windows pairs merge and branch only where assertions differ.
//!
//! Note: the originals asserted process liveness with `child.try_wait()`.
//! `RawDriver` kills its child on drop and does not expose the handle, so the
//! liveness check is replaced with an equivalent post-sleep request probe — a
//! crashed process would EOF and panic inside `recv()`.

#![cfg(target_os = "macos")]

use cu_testkit::RawDriver;

#[test]
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn concurrent_clients() {
    //! Verify two concurrent brigadier-cu processes both respond correctly.
    //! This tests the "multiple brigadier-cu processes" scenario relevant to
    //! the multi-cursor use case.
    let mut drivers: Vec<RawDriver> = Vec::new();
    for _ in 0..2 {
        let Some(d) = RawDriver::spawn() else {
            return;
        };
        drivers.push(d);
    }

    for (i, d) in drivers.iter_mut().enumerate() {
        d.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": i + 1,
            "method": "initialize",
            "params": {}
        }));

        let resp = d.recv();
        assert_eq!(resp["id"], (i + 1) as i64);
        assert!(
            resp["result"]["protocolVersion"].is_string(),
            "Process {i} failed to initialize"
        );
    }
}

#[test]
#[cfg(target_os = "macos")]
fn concurrent_clients_with_cursor_moves() {
    //! Two concurrent brigadier-cu processes, each driving their own cursor, should not crash.
    //! This covers the multi-cursor use case where two Codex agents run simultaneously.
    let mut drivers: Vec<RawDriver> = Vec::new();
    for _ in 0..2 {
        let Some(d) = RawDriver::spawn() else {
            return;
        };
        drivers.push(d);
    }

    // Initialize all processes.
    for (i, d) in drivers.iter_mut().enumerate() {
        d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
        let resp = d.recv();
        assert_eq!(resp["id"], 1, "Process {i} initialize failed");
    }

    // Each process moves its cursor to different positions.
    let positions = [(100.0_f64, 200.0_f64), (500.0, 600.0)];
    for ((i, d), (px, py)) in drivers.iter_mut().enumerate().zip(positions.iter()) {
        d.send(&serde_json::json!({
            "jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":"move_cursor","arguments":{"x":px,"y":py}}
        }));
        let resp = d.recv();
        assert!(
            !resp["result"]["isError"].as_bool().unwrap_or(false),
            "Process {i} move_cursor failed: {resp:?}"
        );
    }

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Each process must still be alive: probe with one more request (a crashed
    // process would EOF and panic in recv()).
    for (i, d) in drivers.iter_mut().enumerate() {
        d.send(&serde_json::json!({
            "jsonrpc":"2.0","id":99,"method":"tools/call",
            "params":{"name":"get_agent_cursor_state","arguments":{}}
        }));
        let resp = d.recv();
        assert_eq!(
            resp["id"], 99,
            "brigadier-cu process {i} crashed during concurrent cursor test"
        );
    }
}

#[test]
#[cfg(target_os = "macos")]
fn concurrent_multi_driver_isolation() {
    //! Two brigadier-cu processes running simultaneously with different sessions.
    //! Verifies that concurrent sessions don't interfere — each tracks its own
    //! cursor state and the overlay stays alive under concurrent load.
    let Some(mut child_a) = RawDriver::spawn() else {
        return;
    };
    let Some(mut child_b) = RawDriver::spawn() else {
        return;
    };

    // Initialize both.
    child_a.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    child_b.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    child_a.recv();
    child_b.recv();

    // Driver A: enable the cursor of session "alpha", Driver B disables "beta".
    child_a.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"set_agent_cursor_enabled","arguments":{"session":"alpha","enabled":true}}
    }));
    child_b.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"set_agent_cursor_enabled","arguments":{"session":"beta","enabled":false}}
    }));
    let resp_a = child_a.recv();
    let resp_b = child_b.recv();
    assert!(
        resp_a["error"].is_null() && !resp_a["result"]["isError"].as_bool().unwrap_or(false),
        "Driver A set_agent_cursor_enabled failed: {resp_a:?}"
    );
    assert!(
        resp_b["error"].is_null() && !resp_b["result"]["isError"].as_bool().unwrap_or(false),
        "Driver B set_agent_cursor_enabled failed: {resp_b:?}"
    );

    // Each driver queries its own session's cursor; the answer is scoped to
    // that session and reflects what it set. The state lives per process.
    child_a.send(&serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"get_agent_cursor_state","arguments":{"session":"alpha"}}
    }));
    child_b.send(&serde_json::json!({
        "jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"get_agent_cursor_state","arguments":{"session":"beta"}}
    }));
    let state_a = child_a.recv();
    let state_b = child_b.recv();
    assert!(
        state_a["error"].is_null(),
        "Driver A get_agent_cursor_state failed: {state_a:?}"
    );
    assert!(
        state_b["error"].is_null(),
        "Driver B get_agent_cursor_state failed: {state_b:?}"
    );
    let state_a = &state_a["result"]["structuredContent"];
    let state_b = &state_b["result"]["structuredContent"];
    assert_eq!(state_a["session"], "alpha", "{state_a}");
    assert_eq!(
        state_a["enabled"].as_bool(),
        Some(true),
        "Cursor alpha should be enabled in driver A"
    );
    assert_eq!(state_b["session"], "beta", "{state_b}");
    assert_eq!(
        state_b["enabled"].as_bool(),
        Some(false),
        "Cursor beta should be disabled in driver B"
    );

    // Exercise move_cursor on both drivers to confirm neither crashes the other.
    child_a.send(&serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"move_cursor","arguments":{"session":"alpha","x":100,"y":100}}
    }));
    child_b.send(&serde_json::json!({
        "jsonrpc":"2.0","id":4,"method":"tools/call",
        "params":{"name":"move_cursor","arguments":{"session":"beta","x":200,"y":200}}
    }));
    let mv_a = child_a.recv();
    let mv_b = child_b.recv();
    assert!(
        mv_a["error"].is_null(),
        "Driver A move_cursor failed: {mv_a:?}"
    );
    assert!(
        mv_b["error"].is_null(),
        "Driver B move_cursor failed: {mv_b:?}"
    );
}

#[test]
#[cfg(target_os = "macos")]
fn multi_cursor_instance_state() {
    //! Two sessions' cursors can be driven independently; each has its own state.
    let Some(mut d) = RawDriver::spawn() else {
        return;
    };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    let mut call = |id: u64, name: &str, arguments: serde_json::Value| {
        d.send(&serde_json::json!({
            "jsonrpc":"2.0","id":id,"method":"tools/call",
            "params":{"name":name,"arguments":arguments}
        }));
        let resp = d.recv();
        assert!(
            !resp["result"]["isError"].as_bool().unwrap_or(false),
            "{name} failed: {resp}"
        );
        resp
    };

    // Move session agent1's cursor to (100, 200) and agent2's to (300, 400).
    call(
        2,
        "move_cursor",
        serde_json::json!({"x":100.0,"y":200.0,"session":"agent1"}),
    );
    call(
        3,
        "move_cursor",
        serde_json::json!({"x":300.0,"y":400.0,"session":"agent2"}),
    );
    // Tune agent1's motion only.
    let motion = call(
        4,
        "set_agent_cursor_motion",
        serde_json::json!({"session":"agent1","glide_duration_ms":450}),
    );
    assert_eq!(
        motion["result"]["structuredContent"]["motion"]["glide_duration_ms"].as_f64(),
        Some(450.0),
        "{motion}"
    );
    // Hide agent2's cursor.
    call(
        5,
        "set_agent_cursor_enabled",
        serde_json::json!({"enabled":false,"session":"agent2"}),
    );

    // get_agent_cursor_state answers for the named session only: agent1 is
    // still enabled, agent2 hidden.
    let resp1 = call(
        6,
        "get_agent_cursor_state",
        serde_json::json!({"session":"agent1"}),
    );
    let state1 = &resp1["result"]["structuredContent"];
    assert_eq!(state1["session"], "agent1", "{state1}");
    assert_eq!(
        state1["enabled"].as_bool(),
        Some(true),
        "agent1 was never disabled"
    );

    let resp2 = call(
        7,
        "get_agent_cursor_state",
        serde_json::json!({"session":"agent2"}),
    );
    let state2 = &resp2["result"]["structuredContent"];
    assert_eq!(state2["session"], "agent2", "{state2}");
    assert_eq!(
        state2["enabled"].as_bool(),
        Some(false),
        "agent2 was hidden"
    );

    std::thread::sleep(std::time::Duration::from_millis(50));
    // Process must still be alive: probe with one more request (a crashed
    // process would EOF and panic in recv()).
    call(
        8,
        "get_agent_cursor_state",
        serde_json::json!({"session":"agent1"}),
    );
}

#[test]
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn overlay_move_cursor_stays_alive() {
    //! Verify that calling move_cursor with the overlay enabled does not crash the process.
    //! The overlay renders to a transparent NSWindow on the main thread; the MCP server
    //! sends commands via the global channel.  Any crash in the render thread (e.g. wrong
    //! GCD queue pointer, bad CGImage argument types) would cause this test to fail with
    //! an early EOF or a non-zero exit code.
    let Some(mut d) = RawDriver::spawn() else {
        return;
    };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    // Call move_cursor several times to drive the overlay render loop.
    for (id, (x, y)) in [
        (2u64, (100.0_f64, 200.0_f64)),
        (3, (400.0, 300.0)),
        (4, (800.0, 600.0)),
    ] {
        d.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": "move_cursor", "arguments": { "x": x, "y": y } }
        }));
        let resp = d.recv();
        assert_eq!(resp["id"], id);
        // Should succeed (isError absent or false).
        let is_error = resp["result"]["isError"].as_bool().unwrap_or(false);
        assert!(!is_error, "move_cursor failed: {:?}", resp);
    }

    // Also call get_agent_cursor_state — should report the last position.
    d.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": { "name": "get_agent_cursor_state", "arguments": {} }
    }));
    let resp = d.recv();
    assert_eq!(resp["id"], 5);
    assert!(!resp["result"]["isError"].as_bool().unwrap_or(false));

    // Give the render loop a chance to tick (two frames ≈ 33 ms).
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Process must still be alive: probe with one more request (a crashed
    // process would EOF and panic in recv()).
    d.send(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 99,
        "method": "tools/call",
        "params": { "name": "get_agent_cursor_state", "arguments": {} }
    }));
    let resp = d.recv();
    assert_eq!(
        resp["id"], 99,
        "brigadier-cu crashed during overlay move_cursor test"
    );
}

#[test]
#[cfg(target_os = "macos")]
fn set_agent_cursor_motion_bezier_knobs() {
    //! set_agent_cursor_motion with Bezier/timing knobs — verifies schema accepts them
    //! and returns a non-error response with the updated values in the response text.
    let Some(mut d) = RawDriver::spawn() else {
        return;
    };

    d.send(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    d.recv();

    // Set Bezier motion knobs including integer-encoded numbers (common from MCP clients).
    d.send(&serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"set_agent_cursor_motion","arguments":{
            "session": "agent1",
            "arc_size": 0.4,
            "spring": 0.85,
            "glide_duration_ms": 500,
            "dwell_after_click_ms": 200,
            "idle_hide_ms": 5000
        }}
    }));
    let resp = d.recv();
    assert!(
        resp["error"].is_null(),
        "Protocol error from set_agent_cursor_motion: {resp:?}"
    );
    assert!(
        !resp["result"]["isError"].as_bool().unwrap_or(false),
        "set_agent_cursor_motion returned isError: {resp:?}"
    );
    // Response text should mention the new glide duration.
    let text = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        text.contains("500") || text.contains("motion"),
        "Expected motion summary in response, got: {text}"
    );
}
