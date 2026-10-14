# Helper integration tests

Tests here exercise the helper's public interface: the stdio MCP protocol,
tool schemas, the daemon socket, and the CLI. Headless protocol tests run by
default. Tests that drive real apps or capture the screen are marked
`#[ignore]`, because they need an interactive desktop and Accessibility and
Screen Recording granted to the test runner.

| Prefix | Runs by default | Purpose |
| --- | --- | --- |
| `protocol_*_test.rs` | yes | MCP and CLI protocol and schema behaviour |
| `schema_*_test.rs` | yes | Schema consistency |
| `installed_app_*_test.rs` | no, `#[ignore]` | Real apps on a signed-in desktop |
